//! A path as it should appear in a message.
//!
//! One function, in a file of its own because the two callers have nothing
//! else to do with each other: `module` shows the path of an imported file,
//! `cmd::upgrade` shows the path of a binary it is about to replace. Each had
//! a `display` of its own, and the two spelled it differently.
//!
//! It is not in `sic-core`. Turning a path into text for a person is a
//! decision about how this program talks, and `sic-core` is the crate that is
//! allowed to know nothing about that.

use std::path::Path;

/// Backslashes become forward slashes so that a path reads the same in a
/// message whichever platform produced it - a diagnostic quoted in an issue
/// should not depend on where it was produced.
///
/// `Display` rather than `to_string_lossy`: it is the one meant for showing a
/// path to a person, and it says so. The two agree on everything sic produces
/// anyway, because every path sic builds comes from text that was already
/// valid UTF-8.
pub fn display(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}
