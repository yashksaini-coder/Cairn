//! Transcript and need-text normalisation.
//!
//! Host-only: the program never sees text. But this still belongs in the
//! shared crate, because the API and the CLI must produce identical bytes,
//! and the TypeScript client mirrors these exact three steps.
//!
//! Without this, the same recording hashes differently depending on whose
//! machine composed the receipt -- macOS hands back NFD, most Linux stacks
//! NFC, and browsers disagree about what a newline in a transcript is.

use unicode_normalization::UnicodeNormalization;

/// NFC-normalise, trim, and collapse every internal whitespace run to a
/// single U+0020.
///
/// The TypeScript equivalent, kept in `web/lib/hash.ts`:
/// `s.normalize("NFC").trim().replace(/\s+/g, " ")`
///
/// JavaScript's `\s` and Rust's `char::is_whitespace` differ on a couple of
/// exotic code points (JS includes U+FEFF, Rust does not), which is why the
/// browser hashes need text for *display verification* only and the server's
/// hash is the one that gets committed.
pub fn normalize_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_space = false;
    for c in raw.nfc() {
        if c.is_whitespace() {
            // Deferred: this drops trailing whitespace for free.
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(c);
    }
    out
}

/// True if `s` is already a fixed point of [`normalize_text`].
pub fn is_normalized(s: &str) -> bool {
    normalize_text(s) == s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_and_trims() {
        assert_eq!(normalize_text("  hello \n\t world  "), "hello world");
        assert_eq!(normalize_text(""), "");
        assert_eq!(normalize_text("   "), "");
    }

    #[test]
    fn composes_decomposed_forms() {
        // "é" as e + U+0301 must equal "é" as U+00E9, or a macOS recipient and
        // a Linux server compute different receipt hashes for one recording.
        assert_eq!(normalize_text("e\u{0301}"), normalize_text("\u{00e9}"));
    }

    #[test]
    fn is_idempotent() {
        let once = normalize_text(" a\u{0301}  b ");
        assert_eq!(normalize_text(&once), once);
        assert!(is_normalized(&once));
    }
}
