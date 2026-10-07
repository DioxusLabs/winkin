//! Which form of a dual-presentation character is wanted.

/// The text or emoji presentation of a cluster.
///
/// Default presentation affects fallback order without restricting named
/// fonts. Selector-forced presentation restricts fonts through
/// [`accepts`](Self::accepts).
///
/// Fallback keys use [`base`](Self::base), so default and forced forms
/// share the same fallback order.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum Presentation {
    /// Default text presentation.
    #[default]
    Text,
    /// Default emoji presentation.
    Emoji,
    /// Text-only presentation, requested by U+FE0E.
    TextOnly,
    /// Emoji-only presentation, requested by U+FE0F or a keycap.
    EmojiOnly,
}

impl Presentation {
    /// Returns the presentation without selector forcing.
    ///
    /// Maps [`TextOnly`](Self::TextOnly) to [`Text`](Self::Text), and
    /// [`EmojiOnly`](Self::EmojiOnly) to [`Emoji`](Self::Emoji).
    pub fn base(self) -> Self {
        match self {
            Self::TextOnly => Self::Text,
            Self::EmojiOnly => Self::Emoji,
            other => other,
        }
    }

    /// Returns `true` if the font is allowed by this presentation.
    ///
    /// Default presentations allow any font. [`TextOnly`](Self::TextOnly)
    /// excludes color fonts; [`EmojiOnly`](Self::EmojiOnly) requires them.
    ///
    /// If no allowed font maps the cluster, the caller may retry using the
    /// default presentation.
    pub fn accepts(self, font: &crate::Font) -> bool {
        match self {
            Self::TextOnly => !font.is_color(),
            Self::EmojiOnly => font.is_color(),
            Self::Text | Self::Emoji => true,
        }
    }
}
