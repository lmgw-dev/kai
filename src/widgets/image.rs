//! `overlay_image` and `overlay_emote` (K19, §10.1): a picture becomes a document the sandboxed
//! frame shows, the picture inside it as a `data:` image (the frame's content filter refuses every
//! network load, so a URL could never load there).
//!
//! **A picture reaches WebKit only after Kai decoded it.** The bytes (fetched, or a `data:` URL's)
//! are decoded as png, jpeg, gif or webp and written out again as a fresh PNG: whatever else the
//! file held (metadata, a polyglot's second format) is gone, and a body that is not an image is
//! refused here with its reason. A gif shows its first frame. The decode is limited to
//! `overlay.image_max_mb` of pixel memory (RGBA), the key named in the error, so a small file that
//! unpacks to a huge picture cannot take the memory. SVG is not accepted (a document can run script;
//! the built-in emotes are Kai's own SVG).

use std::io::Cursor;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use image::{ImageFormat, ImageReader, Limits};

use super::fetch::{self, Policy};
use super::kinds::Body;

/// The built-in emotes: `(name, SVG)`. `contract/overlay.rs`'s description lists the names; a test
/// keeps both in step.
pub const EMOTES: [(&str, &str); 9] = [
    (
        "smile",
        include_str!("../../assets/widgets/emote/smile.svg"),
    ),
    (
        "laugh",
        include_str!("../../assets/widgets/emote/laugh.svg"),
    ),
    ("wink", include_str!("../../assets/widgets/emote/wink.svg")),
    ("sad", include_str!("../../assets/widgets/emote/sad.svg")),
    (
        "surprised",
        include_str!("../../assets/widgets/emote/surprised.svg"),
    ),
    (
        "angry",
        include_str!("../../assets/widgets/emote/angry.svg"),
    ),
    (
        "heart",
        include_str!("../../assets/widgets/emote/heart.svg"),
    ),
    (
        "check",
        include_str!("../../assets/widgets/emote/check.svg"),
    ),
    (
        "question",
        include_str!("../../assets/widgets/emote/question.svg"),
    ),
];

const TOOL: &str = "overlay_image";

/// The document around a picture: it fills the widget, contained, on nothing.
fn page(picture: &str) -> String {
    format!(
        "<!doctype html><meta charset=\"utf-8\"><style>html,body{{margin:0;height:100%;\
         background:transparent}}img,svg{{display:block;width:100%;height:100%;\
         object-fit:contain}}</style>{picture}"
    )
}

/// `overlay_emote {name}`: the name in any case.
pub fn emote(name: &str) -> Result<Body, String> {
    let want = name.trim().to_ascii_lowercase();
    match EMOTES.iter().find(|(n, _)| *n == want) {
        Some((_, svg)) => Ok(Body::Document(page(svg))),
        None => Err(format!(
            "overlay_emote: no emote named {:?}; the names are {}",
            name.trim(),
            EMOTES.map(|(n, _)| n).join(", ")
        )),
    }
}

/// `overlay_image {src}`.
pub async fn image(src: &str, policy: &Policy) -> Result<Body, String> {
    let src = src.trim();
    let bytes = if src
        .get(..5)
        .is_some_and(|p| p.eq_ignore_ascii_case("data:"))
    {
        data_url(src, policy)?
    } else {
        let got = fetch::get(TOOL, src, policy).await?;
        match got.media_type.as_deref() {
            Some(t) if t.starts_with("image/") && t != "image/svg+xml" => {}
            Some("image/svg+xml") => {
                return Err(format!("{TOOL}: SVG is not shown (png, jpeg, gif or webp)"));
            }
            Some(t) => return Err(format!("{TOOL}: the answer is {t}, not an image")),
            None => return Err(format!("{TOOL}: the answer has no Content-Type")),
        }
        got.bytes
    };
    let max_mb = policy.max_mb;
    let png = tokio::task::spawn_blocking(move || reencode(&bytes, max_mb))
        .await
        .map_err(|e| format!("{TOOL}: decoding failed ({e})"))??;
    Ok(Body::Document(page(&format!(
        "<img alt=\"\" src=\"data:image/png;base64,{}\">",
        STANDARD.encode(png)
    ))))
}

/// A base64 `data:` URL's bytes, under the size bound.
fn data_url(src: &str, policy: &Policy) -> Result<Vec<u8>, String> {
    let Some((head, data)) = src[5..].split_once(',') else {
        return Err(format!(
            "{TOOL}: a data: address needs a comma before its data"
        ));
    };
    let mut parts = head.split(';');
    let media = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    if !parts.any(|p| p.trim().eq_ignore_ascii_case("base64")) {
        return Err(format!("{TOOL}: a data: address must be base64"));
    }
    if !media.is_empty() && !media.starts_with("image/") {
        return Err(format!(
            "{TOOL}: the data: address is {media}, not an image"
        ));
    }
    if media == "image/svg+xml" {
        return Err(format!("{TOOL}: SVG is not shown (png, jpeg, gif or webp)"));
    }
    let bytes = STANDARD
        .decode(data.trim())
        .map_err(|e| format!("{TOOL}: the data: address is not valid base64 ({e})"))?;
    if bytes.len() as u64 > policy.max_bytes {
        return Err(format!(
            "{TOOL}: the data: address is larger than overlay.image_max_mb = {}",
            policy.max_mb
        ));
    }
    Ok(bytes)
}

/// Decodes `bytes` as a picture and writes it as a PNG.
pub fn reencode(bytes: &[u8], max_mb: u64) -> Result<Vec<u8>, String> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("{TOOL}: cannot read the picture ({e})"))?;
    match reader.format() {
        Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP) => {}
        _ => {
            return Err(format!(
                "{TOOL}: the data is not a png, jpeg, gif or webp picture"
            ));
        }
    }
    let mut limits = Limits::default();
    limits.max_alloc = Some(max_mb.saturating_mul(1024 * 1024));
    reader.limits(limits);
    let picture = reader.decode().map_err(|e| {
        format!("{TOOL}: the data does not decode as a picture ({e}); pixel memory is bound by overlay.image_max_mb = {max_mb}")
    })?;
    let mut out = Vec::new();
    picture
        .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
        .map_err(|e| format!("{TOOL}: cannot encode the picture ({e})"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::fetch::stand_in::{response, serve};
    use super::*;

    fn policy(max_mb: u64) -> Policy {
        Policy {
            max_bytes: max_mb * 1024 * 1024,
            max_mb,
            private_addresses: true,
            timeout: Duration::from_secs(5),
            timeout_s: 5,
        }
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([61, 174, 233, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    fn doc(b: Body) -> String {
        match b {
            Body::Document(d) => d,
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn a_fetched_picture_is_reencoded_into_a_data_image() {
        let mut with_tail = png(4, 4);
        with_tail.extend_from_slice(b"<script>alert(1)</script>"); // a polyglot's second life
        let s = serve(vec![(
            "/p.png",
            response("200 OK", &[("Content-Type", "image/png")], &with_tail),
        )])
        .await;
        let d = doc(
            image(&format!("http://127.0.0.1:{}/p.png", s.port), &policy(1))
                .await
                .unwrap(),
        );
        assert!(
            d.contains("<img alt=\"\" src=\"data:image/png;base64,"),
            "{d}"
        );
        assert!(
            !d.contains("http://") && !d.contains("alert"),
            "no URL, no tail"
        );
        let b64 = d
            .split("base64,")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let out = STANDARD.decode(b64).unwrap();
        assert_eq!(image::load_from_memory(&out).unwrap().width(), 4);
        assert!(
            !out.windows(8).any(|w| w == b"<script>"),
            "re-encoded, not passed through"
        );
    }

    #[tokio::test]
    async fn a_body_that_is_not_a_picture_is_refused_before_webkit() {
        let s = serve(vec![
            (
                "/html",
                response("200 OK", &[("Content-Type", "text/html")], b"<p>x</p>"),
            ),
            (
                "/lie",
                response(
                    "200 OK",
                    &[("Content-Type", "image/png")],
                    b"<p>not a png</p>",
                ),
            ),
            (
                "/svg",
                response("200 OK", &[("Content-Type", "image/svg+xml")], b"<svg/>"),
            ),
            ("/none", response("200 OK", &[], &png(2, 2))),
            (
                "/trunc",
                response("200 OK", &[("Content-Type", "image/png")], &png(8, 8)[..40]),
            ),
        ])
        .await;
        let base = format!("http://127.0.0.1:{}", s.port);
        for (p, want) in [
            ("html", "text/html, not an image"),
            ("lie", "not a png, jpeg, gif or webp"),
            ("svg", "SVG is not shown"),
            ("none", "no Content-Type"),
            ("trunc", "does not decode"),
        ] {
            let e = image(&format!("{base}/{p}"), &policy(1)).await.unwrap_err();
            assert!(e.contains(want), "{p}: {e}");
            assert!(!e.contains("not a png</p>"), "never the body: {e}");
        }
    }

    #[tokio::test]
    async fn status_and_size_errors_carry_no_body_and_name_the_setting() {
        let s = serve(vec![(
            "/big",
            response(
                "200 OK",
                &[("Content-Type", "image/png")],
                &vec![0u8; 1024 * 1024 + 8],
            ),
        )])
        .await;
        let base = format!("http://127.0.0.1:{}", s.port);
        let e = image(&format!("{base}/big"), &policy(1)).await.unwrap_err();
        assert!(e.contains("overlay.image_max_mb = 1"), "{e}");
        let e = image(&format!("{base}/missing"), &policy(1))
            .await
            .unwrap_err();
        assert!(e.contains("404") && !e.contains("not here"), "{e}");
    }

    #[tokio::test]
    async fn a_small_file_that_unpacks_huge_hits_the_pixel_memory_bound() {
        // 2000 x 2000 RGBA = 16 MB of pixels in a PNG of a few KB.
        let s = serve(vec![(
            "/huge",
            response("200 OK", &[("Content-Type", "image/png")], &png(2000, 2000)),
        )])
        .await;
        let e = image(&format!("http://127.0.0.1:{}/huge", s.port), &policy(4))
            .await
            .unwrap_err();
        assert!(e.contains("overlay.image_max_mb = 4"), "{e}");
        assert!(
            image(&format!("http://127.0.0.1:{}/huge", s.port), &policy(32))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn data_sources_are_checked_like_fetched_ones() {
        let ok = format!("data:image/png;base64,{}", STANDARD.encode(png(3, 3)));
        assert!(doc(image(&ok, &policy(1)).await.unwrap()).contains("data:image/png;base64,"));
        for (src, want) in [
            ("data:image/png,abc", "must be base64"),
            ("data:text/html;base64,PHA+", "not an image"),
            ("data:image/svg+xml;base64,PHN2Zy8+", "SVG is not shown"),
            ("data:image/png;base64,!!!", "not valid base64"),
            ("data:image/png;base64", "needs a comma"),
            (
                "data:image/png;base64,aGVsbG8=",
                "not a png, jpeg, gif or webp",
            ),
        ] {
            let e = image(src, &policy(1)).await.unwrap_err();
            assert!(e.contains(want), "{src}: {e}");
        }
        let big = format!(
            "data:image/png;base64,{}",
            STANDARD.encode(vec![1u8; 1024 * 1024 + 1])
        );
        assert!(
            image(&big, &policy(1))
                .await
                .unwrap_err()
                .contains("overlay.image_max_mb = 1")
        );
        assert!(
            image("ftp://example.invalid/x.png", &policy(1))
                .await
                .unwrap_err()
                .contains("only http and https")
        );
    }

    #[test]
    fn the_emotes_are_listed_in_the_contract_and_are_plain_svg() {
        let (_, spec) = kai_desktop::contract::find("overlay_emote").unwrap();
        let names: Vec<&str> = EMOTES.iter().map(|(n, _)| *n).collect();
        assert!(
            spec.description
                .contains(&format!("({})", names.join(", "))),
            "{}",
            spec.description
        );
        for (name, svg) in EMOTES {
            assert!(
                svg.starts_with("<svg ") && svg.trim_end().ends_with("</svg>"),
                "{name}"
            );
            for banned in [
                "<script",
                "http://www.w3.org/1999",
                "href",
                "<image",
                "onload",
                "<style",
            ] {
                assert!(!svg.contains(banned), "{name}: {banned}");
            }
            // The palette: graphite and blue (plus the light text colour), no pastel.
            for hex in svg.split('#').skip(1).map(|r| &r[..6]) {
                assert!(
                    ["31363b", "3daee9", "eff0f1"].contains(&hex),
                    "{name}: #{hex}"
                );
            }
        }
        let d = doc(emote(" Smile ").unwrap());
        assert!(d.contains("<svg ") && !d.contains("<img"));
        let e = emote("party").unwrap_err();
        assert!(
            e.contains("\"party\"") && e.contains("smile, laugh, wink"),
            "{e}"
        );
    }
}
