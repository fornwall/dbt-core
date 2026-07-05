//! ADBC option keys for the Google Cloud Spanner driver.
//!
//! Matches the `adbc-spanner` driver (<https://github.com/fornwall/adbc-spanner>),
//! which is autocommit-only and authenticates with Application Default
//! Credentials against production Spanner, or anonymous credentials in emulator
//! mode. The database is addressed by a single path option; there are no
//! credential/auth-type options.

/// The Spanner database path: `projects/<p>/instances/<i>/databases/<d>`. Required.
///
/// The driver also accepts this value via the standard `OptionDatabase::Uri`.
pub const DATABASE: &str = "adbc.spanner.database";

/// Explicit gRPC endpoint, e.g. `http://localhost:9010` for an emulator.
pub const ENDPOINT: &str = "adbc.spanner.endpoint";

/// `true` to connect with anonymous credentials (emulator mode). The driver also
/// auto-detects emulator mode from the `SPANNER_EMULATOR_HOST` environment variable.
pub const EMULATOR: &str = "adbc.spanner.emulator";

/// Path to a service-account JSON key file. Overridden by [`KEYFILE_JSON`].
/// When neither is set (and not an emulator) the driver uses Application Default Credentials.
pub const KEYFILE: &str = "adbc.spanner.keyfile";

/// Inline service-account JSON credentials. Takes precedence over [`KEYFILE`].
pub const KEYFILE_JSON: &str = "adbc.spanner.keyfile_json";
