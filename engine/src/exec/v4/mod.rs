//! Eksekusi Uniswap V4.
//!
//! KENAPA TERPISAH DARI V3
//! Bentuk transaksinya berbeda secara mendasar. V3 punya fungsi `mint` dengan
//! satu struct parameter. V4 hanya punya `modifyLiquidities`, yang menerima
//! daftar aksi terkodekan, dan setiap aksi punya bentuk parameternya sendiri.
//! Memaksa keduanya lewat satu builder akan membuat salah satunya dikerjakan
//! setengah hati, dan yang setengah hati itu mengirim uang.
//!
//! INVARIAN
//! - Jalur ini tidak boleh dipakai untuk pool V3, dan sebaliknya. Gate G0 di
//!   `pipeline` yang menegakkannya.
//! - Pool V4 diacu dengan PoolKey, bukan alamat. Dua pool bisa punya currency
//!   dan fee yang sama tapi hook berbeda, dan keduanya pool yang berbeda.

pub mod abi;
pub mod actions;
pub mod calldata;
pub mod permit;
pub mod pipeline;
pub mod verify;
