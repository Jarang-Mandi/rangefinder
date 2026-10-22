//! Penyamaran rahasia di dalam URL RPC.
//!
//! KENAPA MODUL INI ADA
//! RPC publik Robinhood Chain tidak memakai kunci, jadi URL-nya aman ditampilkan
//! apa adanya. Penyedia berbayar tidak begitu. Alchemy, misalnya, menaruh API
//! key langsung di jalur URL:
//!
//! ```text
//! https://robinhood-mainnet.g.alchemy.com/v2/API_KEY_DI_SINI
//! ```
//!
//! URL itu mengalir ke dua tempat yang terlihat: log engine, dan field
//! `endpoint` pada status yang dikirim ke panel. Keduanya mudah ikut tersalin
//! saat user meminta bantuan atau mengirim tangkapan layar, dan siapa pun yang
//! memegang key itu bisa memakai kuota RPC user.
//!
//! INVARIAN
//! - Tidak ada URL RPC yang boleh sampai ke log atau ke panel tanpa lewat sini.
//! - Penyamaran dilakukan dengan MENYIMPAN sedikit, bukan dengan menebak mana
//!   bagian yang rahasia. Menebak berarti suatu saat menebak salah, dan yang
//!   salah itu tidak akan terlihat sampai keynya sudah tersebar.
//! - Host tetap ditampilkan, karena tanpa itu user tidak bisa tahu RPC mana
//!   yang sedang dipakai, dan status kesehatan jadi tidak berguna.

/// Tulisan pengganti untuk bagian yang disembunyikan.
pub const DISAMARKAN: &str = "[disamarkan]";

/// Bentuk URL RPC yang aman ditampilkan.
///
/// Aturannya sengaja kasar: simpan skema, host, dan satu segmen jalur pertama.
/// Sisanya diganti penanda. Query dan info pengguna dibuang seluruhnya, karena
/// keduanya tempat lazim menaruh kunci.
///
/// Akibatnya sebagian URL yang sebenarnya tidak rahasia ikut terpotong, dan itu
/// disengaja. Menampilkan terlalu sedikit hanya merepotkan, menampilkan terlalu
/// banyak membocorkan kunci.
pub fn aman(url: &str) -> String {
    let url = url.trim();
    if url.is_empty() {
        return String::new();
    }

    let (skema, sisa) = match url.split_once("://") {
        Some((s, r)) => (s, r),
        // Tanpa skema, kita tidak bisa memisahkan host dari jalur dengan yakin.
        // Yang paling aman adalah tidak menampilkan apa apa.
        None => return DISAMARKAN.to_string(),
    };

    // Buang query dan fragmen lebih dulu. Keduanya tempat lazim menaruh kunci,
    // dan tidak ada yang perlu dilihat user di sana.
    let sisa = sisa.split(['?', '#']).next().unwrap_or_default();

    // Info pengguna berbentuk `user:sandi@host`. Sandi di situ jelas rahasia,
    // dan menampilkan nama penggunanya pun tidak berguna.
    let sisa = match sisa.rsplit_once('@') {
        Some((_, setelah)) => setelah,
        None => sisa,
    };

    let mut bagian = sisa.split('/');
    let host = bagian.next().unwrap_or_default();
    if host.is_empty() {
        return DISAMARKAN.to_string();
    }

    let jalur: Vec<&str> = bagian.filter(|s| !s.is_empty()).collect();

    match jalur.len() {
        0 => format!("{skema}://{host}"),
        // Satu segmen saja, seperti `/v2`, tidak pernah berisi kunci pada
        // penyedia mana pun yang kita kenal. Ditampilkan supaya user bisa
        // membedakan jalur yang berbeda pada host yang sama.
        1 => format!("{skema}://{host}/{}", jalur[0]),
        _ => format!("{skema}://{host}/{}/{DISAMARKAN}", jalur[0]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_publik_tanpa_kunci_tetap_utuh() {
        assert_eq!(
            aman("https://rpc.mainnet.chain.robinhood.com"),
            "https://rpc.mainnet.chain.robinhood.com"
        );
    }

    #[test]
    fn api_key_alchemy_disamarkan() {
        // Ini kasus yang memicu modul ini. Key-nya tidak boleh muncul.
        let asli = "https://robinhood-mainnet.g.alchemy.com/v2/rahasia_sekali_123";
        let hasil = aman(asli);
        assert_eq!(
            hasil,
            "https://robinhood-mainnet.g.alchemy.com/v2/[disamarkan]"
        );
        assert!(!hasil.contains("rahasia_sekali_123"));
    }

    #[test]
    fn kunci_di_query_dibuang() {
        let hasil = aman("https://contoh.example/rpc?apikey=rahasia&x=1");
        assert!(!hasil.contains("rahasia"));
        assert_eq!(hasil, "https://contoh.example/rpc");
    }

    #[test]
    fn sandi_di_info_pengguna_dibuang() {
        let hasil = aman("https://pengguna:sandirahasia@contoh.example/v1/kunci");
        assert!(!hasil.contains("sandirahasia"));
        assert!(!hasil.contains("pengguna"));
        assert_eq!(hasil, "https://contoh.example/v1/[disamarkan]");
    }

    #[test]
    fn jalur_dalam_hanya_menyisakan_segmen_pertama() {
        assert_eq!(
            aman("https://contoh.example/a/b/c/d"),
            "https://contoh.example/a/[disamarkan]"
        );
    }

    #[test]
    fn websocket_diperlakukan_sama() {
        assert_eq!(
            aman("wss://robinhood-mainnet.g.alchemy.com/v2/kunci"),
            "wss://robinhood-mainnet.g.alchemy.com/v2/[disamarkan]"
        );
    }

    #[test]
    fn url_tanpa_skema_tidak_ditampilkan() {
        // Tanpa skema, memisahkan host dari jalur hanya tebakan, dan tebakan
        // yang salah di sini berarti kunci ikut tercetak.
        assert_eq!(aman("contoh.example/v2/kunci"), DISAMARKAN);
    }

    #[test]
    fn kosong_tetap_kosong() {
        assert_eq!(aman(""), "");
        assert_eq!(aman("   "), "");
    }

    #[test]
    fn garis_miring_di_akhir_tidak_membuat_segmen_palsu() {
        assert_eq!(
            aman("https://rpc.mainnet.chain.robinhood.com/"),
            "https://rpc.mainnet.chain.robinhood.com"
        );
    }
}
