//! Ikatan ABI untuk jalur tulis Uniswap V4.
//!
//! INVARIAN
//! - Berkas ini hanya tanda tangan, TIDAK ADA alamat. Alamat selalu dari
//!   `chain.toml` lewat `chain::registry`.
//! - Setiap tanda tangan di sini WAJIB dibuktikan ada di bytecode yang
//!   terpasang, lewat `cargo xtask v4probe`. Menyalin ABI dari ingatan atau
//!   dari repo Uniswap tidak cukup: rantai ini sudah terbukti memakai fork
//!   pada UniversalRouter, jadi ia bisa berbeda di tempat lain juga.
//!
//! Sumber acuan: Uniswap v4-periphery, `src/PositionManager.sol` dan
//! `src/libraries/Actions.sol`.

use alloy::sol;

sol! {
    #[sol(rpc)]
    #[derive(Debug, PartialEq)]
    interface IPositionManagerV4 {
        /// Kunci pool V4. Bukan alamat, melainkan gabungan lima nilai yang
        /// di-hash menjadi poolId.
        struct PoolKey {
            address currency0;
            address currency1;
            uint24 fee;
            int24 tickSpacing;
            address hooks;
        }

        /// Satu satunya pintu masuk yang mengubah likuiditas di V4.
        ///
        /// `unlockData` adalah `abi.encode(bytes actions, bytes[] params)`.
        /// Tidak ada fungsi `mint` terpisah seperti di V3, jadi seluruh niat
        /// harus dikodekan sebagai daftar aksi.
        function modifyLiquidities(bytes unlockData, uint256 deadline) external payable;

        function nextTokenId() external view returns (uint256);
        function getPositionLiquidity(uint256 tokenId) external view returns (uint128);
        function getPoolAndPositionInfo(uint256 tokenId) external view returns (PoolKey, uint256);
        function poolManager() external view returns (address);
        function permit2() external view returns (address);
        function ownerOf(uint256 tokenId) external view returns (address);
    }
}

sol! {
    #[sol(rpc)]
    interface IPermit2 {
        /// Izin Permit2 ke sebuah spender, dengan masa berlaku.
        ///
        /// V4 PositionManager menarik token lewat Permit2, bukan lewat
        /// allowance ERC20 langsung. Jadi ada dua lapis: token mengizinkan
        /// Permit2, lalu Permit2 mengizinkan PositionManager.
        function approve(address token, address spender, uint160 amount, uint48 expiration) external;

        function allowance(address user, address token, address spender)
            external
            view
            returns (uint160 amount, uint48 expiration, uint48 nonce);
    }
}

sol! {
    /// Dipancarkan PoolManager setiap kali likuiditas berubah.
    ///
    /// Dipakai untuk menemukan transaksi V4 sungguhan yang bisa dijadikan
    /// golden vector, sama seperti `IncreaseLiquidity` pada V3.
    #[derive(Debug)]
    event ModifyLiquidity(
        bytes32 indexed id,
        address indexed sender,
        int24 tickLower,
        int24 tickUpper,
        int256 liquidityDelta,
        bytes32 salt
    );
}
