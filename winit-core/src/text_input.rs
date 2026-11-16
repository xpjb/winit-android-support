//! Synchronous text-editor requests from a platform text input service.
//!
//! Text input systems are not purely event sources. Android `InputConnection`, Apple text input
//! clients, and Windows text stores synchronously query and mutate the focused document. This
//! module models that request/response boundary without requiring a platform-owned text mirror.

use std::ops::Range;

use dpi::{PhysicalPosition, PhysicalSize};

/// Identifies one focused platform text-input connection.
///
/// Platforms may retain an obsolete connection after focus moves. Applications should reject a
/// request whose session ID is no longer active.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TextInputSessionId(u64);

impl TextInputSessionId {
    /// Creates an ID from its platform-independent integer representation.
    pub const fn from_raw(id: u64) -> Self {
        Self(id)
    }

    /// Returns the platform-independent integer representation.
    pub const fn into_raw(self) -> u64 {
        self.0
    }
}

/// A directional selection expressed as UTF-8 byte offsets in the application document.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct TextSelection {
    /// The fixed end of the selection.
    pub anchor: usize,
    /// The active caret end of the selection.
    pub focus: usize,
}

impl TextSelection {
    /// Creates a directional selection.
    pub const fn new(anchor: usize, focus: usize) -> Self {
        Self { anchor, focus }
    }

    /// Returns the ordered byte range covered by the selection.
    pub fn range(self) -> Range<usize> {
        self.anchor.min(self.focus)..self.anchor.max(self.focus)
    }
}

/// Current plain-text state relevant to a platform text input service.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TextInputState {
    /// Document length in UTF-8 bytes.
    pub text_len: usize,
    /// Current directional selection.
    pub selection: TextSelection,
    /// Current composing/marked range, if any.
    pub composition: Option<Range<usize>>,
}

/// One replacement requested by the platform.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextReplacement {
    /// UTF-8 byte range in the application document before the edit.
    pub range: Range<usize>,
    /// Replacement text.
    pub text: String,
}

/// One atomic platform edit.
///
/// The replacement is optional because selection and composition changes are also synchronous text
/// input operations. Selection and composition describe the state after applying the replacement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextInputEdit {
    /// Optional text replacement.
    pub replacement: Option<TextReplacement>,
    /// Selection after the operation.
    pub selection: TextSelection,
    /// Composing/marked range after the operation.
    pub composition: Option<Range<usize>>,
}

/// A document excerpt returned to the platform.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextSnapshot {
    /// Global UTF-8 byte range represented by `text`.
    pub range: Range<usize>,
    /// Plain text in `range`.
    pub text: String,
    /// Current document state.
    pub state: TextInputState,
}

/// A physical rectangle in window-surface coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TextInputRect {
    /// Top-left corner.
    pub origin: PhysicalPosition<f64>,
    /// Rectangle dimensions.
    pub size: PhysicalSize<f64>,
}

impl TextInputRect {
    /// Creates a physical text-input rectangle.
    pub const fn new(origin: PhysicalPosition<f64>, size: PhysicalSize<f64>) -> Self {
        Self { origin, size }
    }
}

/// A synchronous request made by the platform text input service.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum TextInputRequest {
    /// Read a UTF-8 byte range from the focused document.
    Read { range: Range<usize> },
    /// Apply one atomic edit to the focused document.
    Apply(TextInputEdit),
    /// Return the surface rectangle covering a document range.
    RectForRange { range: Range<usize> },
    /// Return the UTF-8 document byte offset nearest a surface position.
    IndexForPoint { position: PhysicalPosition<f64> },
}

/// Result of one platform edit request.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TextInputApplyResult {
    /// The edit was applied and this is the resulting state.
    Applied(TextInputState),
    /// The request was rejected because its indices or operation were invalid.
    Rejected(TextInputState),
    /// The supplied session no longer owns text input focus.
    StaleSession,
}

/// A synchronous answer to a [`TextInputRequest`].
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum TextInputResponse {
    /// Text returned for [`TextInputRequest::Read`].
    Text(TextSnapshot),
    /// Result returned for [`TextInputRequest::Apply`].
    Applied(TextInputApplyResult),
    /// Geometry returned for [`TextInputRequest::RectForRange`].
    Rect(Option<TextInputRect>),
    /// Document index returned for [`TextInputRequest::IndexForPoint`].
    Index(Option<usize>),
    /// The application does not implement this request.
    Unsupported,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directional_selection_preserves_anchor_and_orders_range() {
        let selection = TextSelection::new(9, 3);
        assert_eq!(selection.anchor, 9);
        assert_eq!(selection.focus, 3);
        assert_eq!(selection.range(), 3..9);
    }

    #[test]
    fn session_ids_round_trip_without_hidden_state() {
        let session = TextInputSessionId::from_raw(42);
        assert_eq!(session.into_raw(), 42);
    }
}
