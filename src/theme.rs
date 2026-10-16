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
