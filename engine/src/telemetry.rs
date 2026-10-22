//! Inisialisasi log terstruktur.
//!
//! INVARIAN
//! - Log tidak boleh memuat kunci privat, passphrase, atau token pairing.
//!   Tipe yang membawa rahasia mengimplementasikan Debug secara manual dan
//!   mencetak `[redacted]`. Lihat `wallet::keystore::Secret`.

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Pasang subscriber global. Aman dipanggil sekali saja di awal `main`.
pub fn init() {
    let filter = EnvFilter::try_from_env("RANGEFINDER_LOG")
        .unwrap_or_else(|_| EnvFilter::new("rangefinder=info,tower_http=warn,warn"));

    tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_target(true)
                .with_level(true)
                .with_ansi(true),
        )
        .init();
}
