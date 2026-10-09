//! Kai's tray icons (K9) as pixmaps.
//!
//! The tray sends every icon twice: by name (an installed Kai's hicolor SVGs) and as ARGB32
//! pixmaps rendered from the same SVG files. Plasma's tray uses `IconName` when its icon loader
//! resolves it and falls back to `IconPixmap` otherwise (plasma-workspace
//! `statusnotifieritemsource.cpp`, `loadIcon`). A live check showed why the
//! fallback is needed: in a running plasmashell, names under a directory created after it started,
//! and names under `IconThemePath`, did not resolve, and the item showed the "unknown" icon.

use std::collections::HashMap;
use std::sync::OnceLock;

use resvg::{tiny_skia, usvg};

use super::model::{
    BADGE_HELD, ICON_IDLE, ICON_LISTENING, ICON_MUTED, ICON_OFFLINE, ICON_SPEAKING, ICON_THINKING,
};

macro_rules! status_svg {
    ($name:literal) => {
        include_str!(concat!(
            "../../assets/icons/hicolor/scalable/status/",
            $name,
            ".svg"
        ))
    };
}

const SOURCES: [(&str, &str); 7] = [
    (ICON_OFFLINE, status_svg!("kai-tray-offline")),
    (ICON_IDLE, status_svg!("kai-tray-idle")),
    (ICON_LISTENING, status_svg!("kai-tray-listening")),
    (ICON_THINKING, status_svg!("kai-tray-thinking")),
    (ICON_SPEAKING, status_svg!("kai-tray-speaking")),
    (ICON_MUTED, status_svg!("kai-tray-muted")),
    (BADGE_HELD, status_svg!("kai-tray-held")),
];

/// Panel sizes at scale 1, 1.25, 1.5 and 2; the host picks the closest.
const SIZES: [u32; 5] = [16, 22, 32, 44, 64];

/// The pixmaps for an icon name of the model (empty for a name Kai does not ship).
pub fn pixmaps(name: &str) -> Vec<ksni::Icon> {
    static RENDERED: OnceLock<HashMap<&'static str, Vec<ksni::Icon>>> = OnceLock::new();
    RENDERED
        .get_or_init(|| {
            SOURCES
                .iter()
                .map(|(name, svg)| (*name, render_all(svg)))
                .collect()
        })
        .get(name)
        .cloned()
        .unwrap_or_default()
}

fn render_all(svg: &str) -> Vec<ksni::Icon> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).expect("a shipped SVG parses");
    SIZES.iter().map(|&size| render(&tree, size)).collect()
}

fn render(tree: &usvg::Tree, size: u32) -> ksni::Icon {
    let mut pixmap = tiny_skia::Pixmap::new(size, size).expect("non-zero size");
    let scale = size as f32 / tree.size().width().max(tree.size().height());
    resvg::render(
        tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // SNI wants non-premultiplied ARGB32 in network byte order.
    let data = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.alpha(), c.red(), c.green(), c.blue()]
        })
        .collect();
    ksni::Icon {
        width: size as i32,
        height: size as i32,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argb_at(icon: &ksni::Icon, x: i32, y: i32) -> [u8; 4] {
        let i = ((y * icon.width + x) * 4) as usize;
        icon.data[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn every_icon_renders_at_every_size() {
        for (name, _) in SOURCES {
            let icons = pixmaps(name);
            assert_eq!(icons.len(), SIZES.len(), "{name}");
            for (icon, size) in icons.iter().zip(SIZES) {
                assert_eq!((icon.width, icon.height), (size as i32, size as i32));
                assert_eq!(icon.data.len(), (size * size * 4) as usize);
                // transparent corner, opaque centre
                assert_eq!(argb_at(icon, 0, 0)[0], 0, "{name} {size}: corner");
                assert_eq!(
                    argb_at(icon, icon.width / 2, icon.height / 2)[0],
                    255,
                    "{name} {size}: centre"
                );
            }
        }
        assert!(pixmaps("no-such-icon").is_empty());
    }

    #[test]
    fn the_colours_are_k9s() {
        let at44 = |name| pixmaps(name).into_iter().find(|i| i.width == 44).unwrap();
        // idle: the blue dot in the centre (#3DAEE9)
        assert_eq!(argb_at(&at44(ICON_IDLE), 22, 22), [255, 0x3d, 0xae, 0xe9]);
        // offline: graphite in the centre (#23272C), the grey ring at radius 6.5 of 22 (#8A9199)
        let offline = at44(ICON_OFFLINE);
        assert_eq!(argb_at(&offline, 22, 22), [255, 0x23, 0x27, 0x2c]);
        assert_eq!(argb_at(&offline, 22 + 13, 22), [255, 0x8a, 0x91, 0x99]);
        // held: amber (#E8A33D)
        assert_eq!(argb_at(&at44(BADGE_HELD), 22, 22), [255, 0xe8, 0xa3, 0x3d]);
        // listening: a solid blue disc, blue well off the centre too
        let listening = at44(ICON_LISTENING);
        assert_eq!(argb_at(&listening, 22, 22), [255, 0x3d, 0xae, 0xe9]);
        assert_eq!(argb_at(&listening, 22 + 12, 22), [255, 0x3d, 0xae, 0xe9]);
        // thinking: the middle of three dots; speaking: the middle bar; both blue
        assert_eq!(
            argb_at(&at44(ICON_THINKING), 22, 22),
            [255, 0x3d, 0xae, 0xe9]
        );
        assert_eq!(
            argb_at(&at44(ICON_SPEAKING), 22, 22),
            [255, 0x3d, 0xae, 0xe9]
        );
        // muted: the red stroke through the centre (#DA4453)
        assert_eq!(argb_at(&at44(ICON_MUTED), 22, 22), [255, 0xda, 0x44, 0x53]);
    }
}
