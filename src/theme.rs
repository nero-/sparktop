//! Theme definitions adapted from vllm-top (GPLv3) — mratsim/vllm-top src/theme.rs.
//! Switchable palettes; semantic roles instead of hard-coded colors.
use ratatui::style::Color;

#[derive(Clone, Copy)]
pub struct Theme {
    pub name: &'static str,
    pub bg: Color,
    pub fg: Color,
    pub dim: Color,
    pub accent: Color,
    pub good: Color,
    pub warn: Color,
    pub bad: Color,
    /// primary graph series (decode / CPU)
    pub s1: Color,
    /// secondary graph series (prefill)
    pub s2: Color,
    /// tertiary series (overlays): must contrast strongly with s1
    pub s3: Color,
}

pub const THEMES: [Theme; 5] = [GRUVBOX, CATPPUCCIN, TOKYONIGHT, NORD, DRACULA];

pub const GRUVBOX: Theme = Theme {
    name: "gruvbox",
    bg: Color::Rgb(0x28, 0x28, 0x28),
    fg: Color::Rgb(0xeb, 0xdb, 0xb2),
    dim: Color::Rgb(0xa8, 0x99, 0x84), // bumped from #928374 for contrast (UX review)
    accent: Color::Rgb(0x83, 0xa5, 0x98),
    good: Color::Rgb(0xb8, 0xbb, 0x26),
    warn: Color::Rgb(0xfe, 0x80, 0x19),
    bad: Color::Rgb(0xfb, 0x49, 0x34),
    s1: Color::Rgb(0x8e, 0xc0, 0x7c),
    s2: Color::Rgb(0xd3, 0x86, 0x9b),
    s3: Color::Rgb(0xfa, 0xbd, 0x2f),
};

pub const CATPPUCCIN: Theme = Theme {
    name: "catppuccin",
    bg: Color::Rgb(0x1e, 0x1e, 0x2e),
    fg: Color::Rgb(0xcd, 0xd6, 0xf4),
    dim: Color::Rgb(0x93, 0x99, 0xb2), // bumped from #6c7086 for contrast (UX review)
    accent: Color::Rgb(0x89, 0xb4, 0xfa),
    good: Color::Rgb(0xa6, 0xe3, 0xa1),
    warn: Color::Rgb(0xfa, 0xb3, 0x87),
    bad: Color::Rgb(0xf3, 0x8b, 0xa8),
    s1: Color::Rgb(0x94, 0xe2, 0xd5),
    s2: Color::Rgb(0xf5, 0xc2, 0xe7),
    s3: Color::Rgb(0xf9, 0xe2, 0xaf),
};

pub const TOKYONIGHT: Theme = Theme {
    name: "tokyonight",
    bg: Color::Rgb(0x1a, 0x1b, 0x26),
    fg: Color::Rgb(0xc0, 0xca, 0xf5),
    dim: Color::Rgb(0x73, 0x7a, 0xa2), // bumped from #565f89 for contrast (UX review)
    accent: Color::Rgb(0x7a, 0xa2, 0xf7),
    good: Color::Rgb(0x9e, 0xce, 0x6a),
    warn: Color::Rgb(0xe0, 0xaf, 0x68),
    bad: Color::Rgb(0xf7, 0x76, 0x8e),
    s1: Color::Rgb(0x73, 0xda, 0xca),
    s2: Color::Rgb(0xbb, 0x9a, 0xf7),
    s3: Color::Rgb(0xff, 0x9e, 0x64),
};

pub const NORD: Theme = Theme {
    name: "nord",
    bg: Color::Rgb(0x2e, 0x34, 0x40),
    fg: Color::Rgb(0xec, 0xef, 0xf4),
    dim: Color::Rgb(0x8f, 0x9b, 0xb3),
    accent: Color::Rgb(0x88, 0xc0, 0xd0),
    good: Color::Rgb(0xa3, 0xbe, 0x8c),
    warn: Color::Rgb(0xeb, 0xcb, 0x8b),
    bad: Color::Rgb(0xbf, 0x61, 0x6a),
    s1: Color::Rgb(0x8f, 0xbc, 0xbb),
    s2: Color::Rgb(0xb4, 0x8e, 0xad),
    s3: Color::Rgb(0xd0, 0x87, 0x70),
};

pub const DRACULA: Theme = Theme {
    name: "dracula",
    bg: Color::Rgb(0x28, 0x2a, 0x36),
    fg: Color::Rgb(0xf8, 0xf8, 0xf2),
    dim: Color::Rgb(0x8b, 0x95, 0xc9),
    accent: Color::Rgb(0xbd, 0x93, 0xf9),
    good: Color::Rgb(0x50, 0xfa, 0x7b),
    warn: Color::Rgb(0xff, 0xb8, 0x6c),
    bad: Color::Rgb(0xff, 0x55, 0x55),
    s1: Color::Rgb(0x8b, 0xe9, 0xfd),
    s2: Color::Rgb(0xff, 0x79, 0xc6),
    s3: Color::Rgb(0xf1, 0xfa, 0x8c),
};

pub fn theme_index(name: &str) -> Option<usize> {
    THEMES.iter().position(|t| t.name.eq_ignore_ascii_case(name))
}

impl Theme {
    /// Distinct per-node series colors for overlay (compare) charts.
    pub fn series(&self, i: usize) -> Color {
        let p = [self.s1, self.s3, self.s2, self.accent, self.good, self.warn, self.bad, self.fg];
        p[i % p.len()]
    }

    /// Subtle row highlight for the focused table row: fg blended into bg.
    pub fn highlight(&self) -> Color {
        lerp(self.bg, self.fg, 0.12)
    }
}

/// Linear blend between two RGB colors; non-RGB colors snap at the midpoint.
pub fn lerp(a: Color, b: Color, t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let m = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
            Color::Rgb(m(r1, r2), m(g1, g2), m(b1, b2))
        }
        _ => if t < 0.5 { a } else { b },
    }
}

/// Color ramp for usage-style ratios: calm when healthy, loud near full.
pub fn usage_color(t: &Theme, ratio: f64) -> Color {
    if ratio < 0.6 {
        t.s1
    } else if ratio < 0.85 {
        t.warn
    } else {
        t.bad
    }
}
