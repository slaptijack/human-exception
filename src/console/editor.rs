//! The player's editing/cursor-movement vocabulary for the Controller
//! source pane.
//!
//! `EditOp` is the payload of `Msg::EditController`; `super::document`
//! dispatches it onto the embedded editor foundation (issue #90) that is
//! now the sole authoritative working buffer.

/// One editing or cursor-movement operation the player can perform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOp {
    Insert(char),
    Newline,
    Backspace,
    DeleteForward,
    MoveLeft,
    MoveRight,
    MoveUp,
    MoveDown,
    MoveLineStart,
    MoveLineEnd,
    PageUp,
    PageDown,
}
