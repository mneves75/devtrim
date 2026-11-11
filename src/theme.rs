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
//! `NO_COLOR` set every token degrades to a modifier rather than a color. The
//! guarantee is deliberately narrow and worth stating exactly: the danger
//! ladder stays mutually distinct AND ordered, because severity is the one
//! thing no surrounding word repeats. Semantic tokens may share a modifier —
//! `Success` and `Info` both render plain — since the text beside them already
//! names the state ("READ-ONLY", "No findings"). Claiming every token stays
//! distinct would be false, and a false claim here is worse than a narrow one.

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
    /// A label that must stand out from the metadata beside it, in the
    /// terminal's own foreground.
    Strong,
    /// The header bar that separates the screen's chrome from its content: a
    /// step in tone, never a line, and never a color, so it reads the same in
    /// every terminal theme and under `NO_COLOR`.
    Bar,
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
        match (self.support, color_of(token)) {
            (ColorSupport::Named, Some(color)) => Style::default().fg(color),
            (ColorSupport::Named, None) | (ColorSupport::None, _) => {
                Style::default().add_modifier(modifier_of(token))
            }
        }
    }

    /// Same token, additionally emphasized. Under `NO_COLOR` the token's own
    /// modifier already carries the meaning, so this only adds weight.
    pub fn bold(self, token: Token) -> Style {
        self.style(token).add_modifier(Modifier::BOLD)
    }
}

/// `None` for the tokens drawn in the terminal's own colors, which use their
/// monochrome modifier everywhere.
fn color_of(token: Token) -> Option<Color> {
    Some(match token {
        Token::Accent | Token::Success | Token::DangerLow => Color::Green,
        Token::AccentSecondary => Color::Cyan,
        Token::Muted => Color::DarkGray,
        Token::Warning | Token::DangerModerate => Color::Yellow,
        Token::Critical | Token::DangerCritical => Color::Red,
        Token::Info => Color::Blue,
        Token::DangerHigh => Color::LightRed,
        Token::Strong | Token::Bar => return None,
    })
}

/// Monochrome fallbacks. The danger ladder stays ordered — dim, plain, bold,
/// bold+reversed — so severity remains readable with color stripped entirely.
fn modifier_of(token: Token) -> Modifier {
    match token {
        Token::Accent | Token::AccentSecondary | Token::Strong => Modifier::BOLD,
        Token::Muted | Token::DangerLow => Modifier::DIM,
        Token::Success | Token::Info | Token::DangerModerate => Modifier::empty(),
        Token::Warning | Token::DangerHigh => Modifier::BOLD,
        Token::Critical | Token::DangerCritical => Modifier::BOLD | Modifier::REVERSED,
        Token::Bar => Modifier::REVERSED,
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

    /// Severity is the one meaning no surrounding word repeats, so with color
    /// stripped the four danger levels must still read in order: each adds
    /// weight to the one below it.
    #[test]
    fn the_monochrome_danger_ladder_is_strictly_ordered() {
        let theme = Theme::new(ColorSupport::None);
        let ladder = [
            Token::DangerLow,
            Token::DangerModerate,
            Token::DangerHigh,
            Token::DangerCritical,
        ]
        .map(|token| theme.style(token));
        for style in ladder {
            assert_eq!(style.fg, None, "NO_COLOR painted {style:?}");
            assert_eq!(style.bg, None, "NO_COLOR painted {style:?}");
        }
        let [low, moderate, high, critical] = ladder.map(|style| style.add_modifier);
        assert_eq!(low, Modifier::DIM);
        assert_eq!(moderate, Modifier::empty());
        assert_eq!(high, Modifier::BOLD);
        assert_eq!(critical, Modifier::BOLD | Modifier::REVERSED);
    }

    /// The token boundaries follow the 1-10 danger scale: 2, 5 and 8 are the
    /// last score of each level.
    #[test]
    fn danger_scores_map_to_their_level_at_each_boundary() {
        for (score, token) in [
            (0, Token::DangerLow),
            (2, Token::DangerLow),
            (3, Token::DangerModerate),
            (5, Token::DangerModerate),
            (6, Token::DangerHigh),
            (8, Token::DangerHigh),
            (9, Token::DangerCritical),
            (10, Token::DangerCritical),
        ] {
            assert_eq!(danger_token(score), token, "score {score}");
        }
    }

    /// The chrome tokens never carry a color, in either support level, so the
    /// bar reads as a step in tone in every terminal theme.
    #[test]
    fn chrome_tokens_use_the_terminal_colors() {
        for support in [ColorSupport::Named, ColorSupport::None] {
            let theme = Theme::new(support);
            assert_eq!(theme.style(Token::Bar).fg, None);
            assert_eq!(theme.style(Token::Bar).add_modifier, Modifier::REVERSED);
            assert_eq!(theme.style(Token::Strong).add_modifier, Modifier::BOLD);
        }
        assert!(
            Theme::new(ColorSupport::Named)
                .style(Token::Muted)
                .fg
                .is_some()
        );
    }
}
