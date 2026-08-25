//! The player's editing/cursor-movement vocabulary for the Controller
//! source pane.
//!
//! `EditOp` is the payload of `Msg::EditController`; `super::document`
//! dispatches it onto the embedded editor foundation (issue #90) that is
//! now the sole authoritative working buffer. Movement variants carry a
//! `shift: bool` payload matching the library's own `MoveLeft { shift }`-
//! style actions: `true` extends the current selection to the new cursor
//! position, `false` moves the cursor and clears any active selection.

/// One editing or cursor-movement operation the player can perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOp {
    Insert(char),
    Newline,
    Backspace,
    DeleteForward,
    MoveLeft(bool),
    MoveRight(bool),
    MoveUp(bool),
    MoveDown(bool),
    MoveLineStart(bool),
    MoveLineEnd(bool),
    PageUp(bool),
    PageDown(bool),
    MoveWordLeft(bool),
    MoveWordRight(bool),
    SelectAll,
    Undo,
    Redo,
    /// `Tab`: indents the current line, or every line touched by an active
    /// selection, by one language-appropriate indent unit (two spaces for
    /// Lua, never a literal tab byte).
    Indent,
    /// `Shift+Tab`: removes one indent unit from the current line, or every
    /// line touched by an active selection.
    UnIndent,
}
