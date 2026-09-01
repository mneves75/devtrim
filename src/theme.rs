//! Semantic style tokens for the terminal interface.
//!
//! Two rules drive this module.
//!
//! First, colors are named ANSI colors, never RGB. Named colors resolve through
//! the user's own terminal theme, so devtrim stays legible in a light theme, a
//! high-contrast theme, and over SSH. A fixed RGB palette would look identical
//! everywhere at the cost of matching nowhere.
//!
//! Second, color is an enhancement and never the only carrier of meaning. With
//! `NO_COLOR` set, every token degrades to a modifier that preserves the same
//! distinction, so the interface stays usable with no color at all rather than
//! collapsing into undifferentiated text.

use ratatui::style::{Color, Modifier, Style};

/// Meaning, not appearance. Call sites name what a span *is*; this module is the
/// only place that decides how that looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    /// Product identity and the current selection.
    Accent,
    /// Secondary emphasis: keys and inline hints.
    AccentSecondary,
    /// Metadata that must not compete with content.
    Muted,
    /// A safe or completed state.
    Success,
    /// Something the operator should read before continuing.
    Warning,
    /// Destructive or failed.
    Critical,
    /// Informational classification, carrying no risk.
    Info,
    /// Danger ladder, mirroring the 1-10 finding score.
    DangerLow,
    DangerModerate,
    DangerHigh,
    DangerCritical,
}

/// Whether color may carry meaning at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorSupport {
    /// Named ANSI colors, resolved by the terminal's own theme.
    Named,
    /// `NO_COLOR` is set: modifiers only.
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    support: ColorSupport,
}

impl Theme {
    /// Honors `NO_COLOR` unconditionally: the convention is that any value,
    /// including an empty one, disables color.
    pub fn from_env() -> Self {
        Self::new(if std::env::var_os("NO_COLOR").is_some() {
            ColorSupport::None
        } else {
            ColorSupport::Named
        })
    }

    pub fn new(support: ColorSupport) -> Self {
        Self { support }
    }

    pub fn style(self, token: Token) -> Style {
        match self.support {
            ColorSupport::Named => Style::default().fg(color_of(token)),
            ColorSupport::None => Style::default().add_modifier(modifier_of(token)),
        }
    }

    /// Same token, additionally emphasized. Under `NO_COLOR` the token's own
    /// modifier already carries the meaning, so this only adds weight.
    pub fn bold(self, token: Token) -> Style {
        self.style(token).add_modifier(Modifier::BOLD)
    }
}

fn color_of(token: Token) -> Color {
    match token {
        Token::Accent | Token::Success | Token::DangerLow => Color::Green,
        Token::AccentSecondary => Color::Cyan,
        Token::Muted => Color::DarkGray,
        Token::Warning | Token::DangerModerate => Color::Yellow,
        Token::Critical | Token::DangerCritical => Color::Red,
        Token::Info => Color::Blue,
        Token::DangerHigh => Color::LightRed,
    }
}

/// Monochrome fallbacks. The danger ladder stays ordered — dim, plain, bold,
/// bold+reversed — so severity remains readable with color stripped entirely.
fn modifier_of(token: Token) -> Modifier {
    match token {
        Token::Accent | Token::AccentSecondary => Modifier::BOLD,
        Token::Muted | Token::DangerLow => Modifier::DIM,
        Token::Success | Token::Info | Token::DangerModerate => Modifier::empty(),
        Token::Warning | Token::DangerHigh => Modifier::BOLD,
        Token::Critical | Token::DangerCritical => Modifier::BOLD | Modifier::REVERSED,
    }
}

/// Danger score to token, mirroring the 1-10 scale used by findings and plans.
pub fn danger_token(danger: u8) -> Token {
    match danger {
        0..=2 => Token::DangerLow,
        3..=5 => Token::DangerModerate,
        6..=8 => Token::DangerHigh,
        _ => Token::DangerCritical,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVERY_TOKEN: &[Token] = &[
        Token::Accent,
        Token::AccentSecondary,
        Token::Muted,
        Token::Success,
        Token::Warning,
        Token::Critical,
        Token::Info,
        Token::DangerLow,
        Token::DangerModerate,
        Token::DangerHigh,
        Token::DangerCritical,
    ];

    #[test]
    fn colored_theme_sets_a_foreground_and_never_a_modifier_only() {
        let theme = Theme::new(ColorSupport::Named);
        for token in EVERY_TOKEN {
            assert!(
                theme.style(*token).fg.is_some(),
                "{token:?} must carry a named color"
            );
        }
    }

    /// The point of the monochrome mode: no token may set a color, and the
    /// interface must not depend on one.
    #[test]
    fn monochrome_theme_never_sets_a_color() {
        let theme = Theme::new(ColorSupport::None);
        for token in EVERY_TOKEN {
            let style = theme.style(*token);
            assert!(style.fg.is_none(), "{token:?} must not set a color");
            assert!(style.bg.is_none(), "{token:?} must not set a background");
        }
    }

    /// Positive control for the fallback: if every token degraded to the same
    /// empty style the monochrome assertion above would still pass while the
    /// interface became unreadable, so severity must stay distinguishable.
    #[test]
    fn monochrome_danger_ladder_stays_ordered_and_distinct() {
        let theme = Theme::new(ColorSupport::None);
        let ladder: Vec<Modifier> = [0u8, 4, 7, 10]
            .iter()
            .map(|danger| theme.style(danger_token(*danger)).add_modifier)
            .collect();
        for (index, modifier) in ladder.iter().enumerate() {
            for other in ladder.iter().skip(index + 1) {
                assert_ne!(modifier, other, "danger levels must stay distinguishable");
            }
        }
    }

    #[test]
    fn danger_token_covers_the_whole_scale() {
        assert_eq!(danger_token(0), Token::DangerLow);
        assert_eq!(danger_token(2), Token::DangerLow);
        assert_eq!(danger_token(3), Token::DangerModerate);
        assert_eq!(danger_token(5), Token::DangerModerate);
        assert_eq!(danger_token(6), Token::DangerHigh);
        assert_eq!(danger_token(8), Token::DangerHigh);
        assert_eq!(danger_token(9), Token::DangerCritical);
        assert_eq!(danger_token(u8::MAX), Token::DangerCritical);
    }

    #[test]
    fn bold_preserves_the_token_meaning() {
        let theme = Theme::new(ColorSupport::Named);
        assert_eq!(
            theme.bold(Token::Accent).fg,
            theme.style(Token::Accent).fg,
            "emphasis must not change which token is being shown"
        );
        assert!(
            theme
                .bold(Token::Accent)
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }
}
