//! A capture for the model: PNG (lossless, so text stays legible; one of the formats llama.cpp
//! decodes, egress §8.2), RGB where every pixel is opaque and RGBA where not, at the capture's own
//! size, base64 for MCP's image content.

use base64::Engine;
use kai_desktop::caps::capture::Pixels;

/// `pixels` as a base64 PNG.
pub fn png_base64(pixels: &Pixels) -> Result<String, String> {
    let png = png(pixels)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(png))
}

/// `pixels` as a PNG file's bytes.
pub fn png(pixels: &Pixels) -> Result<Vec<u8>, String> {
    let (w, h) = (pixels.width, pixels.height);
    let expected = w as usize * h as usize * 4;
    if w == 0 || h == 0 || pixels.rgba.len() != expected {
        return Err(format!(
            "the capture holds {} bytes for {w}x{h} pixels, so Kai cannot encode it",
            pixels.rgba.len()
        ));
    }
    let opaque = pixels.opaque();
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(if opaque {
        png::ColorType::Rgb
    } else {
        png::ColorType::Rgba
    });
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Balanced);
    let mut writer = enc
        .write_header()
        .map_err(|e| format!("Kai could not encode the capture: {e}"))?;
    let rgb;
    let data: &[u8] = if opaque {
        rgb = pixels
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect::<Vec<u8>>();
        &rgb
    } else {
        &pixels.rgba
    };
    writer
        .write_image_data(data)
        .map_err(|e| format!("Kai could not encode the capture: {e}"))?;
    writer
        .finish()
        .map_err(|e| format!("Kai could not encode the capture: {e}"))?;
    Ok(out)
}
