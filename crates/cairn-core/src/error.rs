//! Errors that can arise inside the shared core. Program-level errors live in
//! `programs/cairn/src/errors.rs`; these are decode-side failures the host
//! tooling hits.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CairnError {
    /// The account exists but was not written by this program.
    BadDiscriminator,
    /// Right discriminator, unreadable body -- a program/indexer version skew.
    BadAccountData,
    /// Text failed the normalisation contract (currently: nothing does, but
    /// callers should still handle it rather than unwrap).
    NotNormalized,
    /// A state name that is not one of `funded`, `released`, `refunded`.
    UnknownState,
}

impl core::fmt::Display for CairnError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let s = match self {
            CairnError::BadDiscriminator => "account discriminator does not belong to Cairn",
            CairnError::BadAccountData => "escrow account data could not be decoded",
            CairnError::NotNormalized => "text is not in canonical normalised form",
            CairnError::UnknownState => "not a Cairn escrow state",
        };
        f.write_str(s)
    }
}

#[cfg(feature = "host")]
impl std::error::Error for CairnError {}
