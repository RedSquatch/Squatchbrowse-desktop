//! Colour schemes for the toolbar and SquatchBrowse's own pages (home, settings, errors).

/// (id, name, [bg, bg2, bg3, fg, muted, accent])
pub const THEMES: &[(&str, &str, [&str; 6])] = &[
    ("midnight", "Midnight", ["#1b1d20", "#25282c", "#2f3338", "#e4e1da", "#8f8b84", "#e0a85a"]),
    ("brass", "Mahogany & brass", ["#1d0d07", "#2b140b", "#3b1d12", "#f1e6cb", "#b39b7a", "#ecc97f"]),
    ("forest", "Forest", ["#111915", "#1a2620", "#24332a", "#e2e8df", "#8fa393", "#7cc48a"]),
    ("ocean", "Deep ocean", ["#0d141e", "#152030", "#1e2c41", "#e1e8f0", "#8697ab", "#3ab8e8"]),
    ("plum", "Plum", ["#17121c", "#211a29", "#2c2336", "#ebe3f0", "#9a8ca6", "#c792ea"]),
    ("ember", "Ember", ["#1a1110", "#261816", "#33201d", "#f0e4df", "#a68f88", "#ff7a45"]),
    ("slate", "Slate", ["#15181c", "#1e2227", "#292e35", "#dfe3e8", "#8a939e", "#9ab0c8"]),
    ("paper", "Paper (light)", ["#f4f2ee", "#ffffff", "#e6e2da", "#1f1d1a", "#6d675e", "#b5651d"]),
];

pub fn colours(id: &str) -> [&'static str; 6] {
    THEMES.iter().find(|t| t.0 == id).unwrap_or(&THEMES[0]).2
}

/// CSS that sets the page colour variables for a theme.
pub fn css(id: &str) -> String {
    let c = colours(id);
    format!(
        ":root{{--bg:{};--bg2:{};--bg3:{};--fg:{};--muted:{};--accent:{};--line:{};}}",
        c[0], c[1], c[2], c[3], c[4], c[5], c[2]
    )
}
