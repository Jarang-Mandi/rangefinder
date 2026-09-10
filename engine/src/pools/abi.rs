//! Ikatan ABI kontrak yang dibaca engine.
//!
//! INVARIAN
//! - Berkas ini hanya berisi tanda tangan fungsi, TIDAK ADA alamat. Alamat
//!   selalu datang dari `chain.toml` lewat `chain::registry`.
//! - Hanya fungsi baca yang didefinisikan di sini. Fungsi yang mengubah state
//!   masuk ke modul eksekusi di fase 3, supaya tidak ada jalur tulis yang
//!   tidak sengaja terpakai sebelum gate validasi lengkap.

use alloy::sol;

sol! {
    #[sol(rpc)]
    interface IERC20 {
        function decimals() external view returns (uint8);
        function totalSupply() external view returns (uint256);
        function symbol() external view returns (string);
        function name() external view returns (string);
        function balanceOf(address account) external view returns (uint256);
    }
}

sol! {
    #[sol(rpc)]
    interface IUniswapV3Factory {
        function getPool(address tokenA, address tokenB, uint24 fee) external view returns (address);
    }
}

sol! {
    #[sol(rpc)]
    interface IUniswapV3Pool {
        function slot0() external view returns (
            uint160 sqrtPriceX96,
            int24 tick,
            uint16 observationIndex,
            uint16 observationCardinality,
            uint16 observationCardinalityNext,
            uint8 feeProtocol,
            bool unlocked
        );
        function liquidity() external view returns (uint128);
        function token0() external view returns (address);
        function token1() external view returns (address);
        function fee() external view returns (uint24);
        function tickSpacing() external view returns (int24);

        /// Bitmap tick terinisialisasi, satu kata untuk 256 tick terkompresi.
        ///
        /// Bentuknya sama dengan padanan V4, hanya tempatnya berbeda: di V3
        /// datanya ada di kontrak pool itu sendiri, di V4 di PoolManager yang
        /// dibaca lewat StateView.
        function tickBitmap(int16 wordPosition) external view returns (uint256);

        /// Keadaan satu tick. Yang dipakai hanya dua field pertama, sisanya
        /// ikut karena bentuk kembaliannya memang begitu dan memotongnya akan
        /// membuat penguraiannya gagal.
        function ticks(int24 tick) external view returns (
            uint128 liquidityGross,
            int128 liquidityNet,
            uint256 feeGrowthOutside0X128,
            uint256 feeGrowthOutside1X128,
            int56 tickCumulativeOutside,
            uint160 secondsPerLiquidityOutsideX128,
            uint32 secondsOutside,
            bool initialized
        );
    }
}

sol! {
    #[sol(rpc)]
    interface IStateView {
        function getSlot0(bytes32 poolId) external view returns (
            uint160 sqrtPriceX96,
            int24 tick,
            uint24 protocolFee,
            uint24 lpFee
        );
        function getLiquidity(bytes32 poolId) external view returns (uint128);

        /// Likuiditas yang ditambahkan dan dikurangi tepat di satu tick.
        ///
        /// `liquidityNet` adalah kunci menyusun distribusi: berjalan dari tick
        /// sekarang ke luar sambil menjumlahkannya menghasilkan likuiditas aktif
        /// di setiap pita. Tanpa itu, yang bisa dilihat hanya likuiditas di satu
        /// titik, dan satu titik tidak memberi tahu apa apa soal bentuk pool.
        function getTickLiquidity(bytes32 poolId, int24 tick) external view returns (
            uint128 liquidityGross,
            int128 liquidityNet
        );

        /// Satu kata bitmap, 256 tick terkompresi per kata.
        ///
        /// Dipakai mencari tick mana saja yang terinisialisasi. Tanpa bitmap,
        /// menemukannya berarti mencoba setiap tick satu per satu, dan rentang
        /// tick V4 mencakup ratusan ribu kemungkinan.
        function getTickBitmap(bytes32 poolId, int16 tick) external view returns (uint256);
    }
}

sol! {
    /// Event `Initialize` dari PoolManager V4.
    ///
    /// Ini satu satunya cara menemukan pool V4, karena V4 tidak punya factory
    /// yang bisa dienumerasi seperti V3. `id`, `currency0`, dan `currency1`
    /// terindeks, jadi pencarian bisa disaring di sisi RPC dan tetap murah.
    #[derive(Debug)]
    event Initialize(
        bytes32 indexed id,
        address indexed currency0,
        address indexed currency1,
        uint24 fee,
        int24 tickSpacing,
        address hooks,
        uint160 sqrtPriceX96,
        int24 tick
    );
}

sol! {
    /// Event `Swap` dari PoolManager V4.
    ///
    /// `id` terindeks, jadi volume per pool bisa dihitung tanpa satu permintaan
    /// per pool: satu pemindaian atas seluruh PoolManager, lalu dikelompokkan
    /// di sini menurut poolId.
    ///
    /// `amount0` dan `amount1` bertanda, dan tandanya dari sudut pandang pool.
    /// Yang dipakai sebagai ukuran volume adalah nilai mutlaknya.
    #[derive(Debug)]
    event Swap(
        bytes32 indexed id,
        address indexed sender,
        int128 amount0,
        int128 amount1,
        uint160 sqrtPriceX96,
        uint128 liquidity,
        int24 tick,
        uint24 fee
    );
}

sol! {
    /// Event `Swap` dari kontrak pool V3.
    ///
    /// Bentuknya berbeda dari padanan V4: tidak ada `id`, karena pool yang
    /// bersangkutan adalah kontrak yang memancarkannya. Yang mengenali pool
    /// adalah alamat log, bukan salah satu topic.
    ///
    /// `amount0` dan `amount1` di sini `int256`, bukan `int128`. Memakai ABI
    /// V4 untuk log V3 akan gagal diurai tanpa pesan yang menjelaskan apa pun.
    ///
    /// NAMANYA HARUS TETAP `Swap`
    /// Topic sebuah event dihitung dari NAMA beserta tipe argumennya. Menamainya
    /// `SwapV3` supaya tidak bentrok dengan padanan V4 menghasilkan topic yang
    /// tidak cocok dengan apa pun di rantai, dan akibatnya bukan error melainkan
    /// nol log yang terbaca sebagai pool yang sepi. Bentroknya diselesaikan
    /// dengan menaruhnya di dalam interface tersendiri, bukan dengan mengganti
    /// namanya.
    interface IUniswapV3Events {
        #[derive(Debug)]
        event Swap(
            address indexed sender,
            address indexed recipient,
            int256 amount0,
            int256 amount1,
            uint160 sqrtPriceX96,
            uint128 liquidity,
            int24 tick
        );
    }
}
