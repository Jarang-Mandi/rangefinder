//! Ikatan ABI sisi tulis, yaitu fungsi yang mengubah state.
//!
//! INVARIAN
//! - Dipisah dari `pools::abi` yang hanya membaca, supaya jelas mana jalur yang
//!   bisa memindahkan dana. Jangan pernah menambahkan fungsi tulis ke modul
//!   baca, atau sebaliknya.
//! - Tidak ada alamat di berkas ini. Semua alamat lewat `chain::registry`.
//! - Struct di sini harus sama persis dengan kontraknya, termasuk urutan field,
//!   karena urutan field menentukan tata letak calldata. Field yang tertukar
//!   menghasilkan calldata yang tetap sah bentuknya tapi salah artinya, dan itu
//!   jenis kesalahan yang hanya tertangkap golden vector.
//!
//! Sumber: Uniswap v3-periphery, `contracts/NonfungiblePositionManager.sol`.

use alloy::sol;

sol! {
    #[sol(rpc)]
    interface INonfungiblePositionManager {
        struct MintParams {
            address token0;
            address token1;
            uint24 fee;
            int24 tickLower;
            int24 tickUpper;
            uint256 amount0Desired;
            uint256 amount1Desired;
            uint256 amount0Min;
            uint256 amount1Min;
            address recipient;
            uint256 deadline;
        }

        struct IncreaseLiquidityParams {
            uint256 tokenId;
            uint256 amount0Desired;
            uint256 amount1Desired;
            uint256 amount0Min;
            uint256 amount1Min;
            uint256 deadline;
        }

        struct DecreaseLiquidityParams {
            uint256 tokenId;
            uint128 liquidity;
            uint256 amount0Min;
            uint256 amount1Min;
            uint256 deadline;
        }

        struct CollectParams {
            uint256 tokenId;
            address recipient;
            uint128 amount0Max;
            uint128 amount1Max;
        }

        function mint(MintParams params)
            external
            payable
            returns (uint256 tokenId, uint128 liquidity, uint256 amount0, uint256 amount1);

        function increaseLiquidity(IncreaseLiquidityParams params)
            external
            payable
            returns (uint128 liquidity, uint256 amount0, uint256 amount1);

        function decreaseLiquidity(DecreaseLiquidityParams params)
            external
            payable
            returns (uint256 amount0, uint256 amount1);

        function collect(CollectParams params)
            external
            payable
            returns (uint256 amount0, uint256 amount1);

        function burn(uint256 tokenId) external payable;

        function multicall(bytes[] data) external payable returns (bytes[] results);

        function positions(uint256 tokenId)
            external
            view
            returns (
                uint96 nonce,
                address operator,
                address token0,
                address token1,
                uint24 fee,
                int24 tickLower,
                int24 tickUpper,
                uint128 liquidity,
                uint256 feeGrowthInside0LastX128,
                uint256 feeGrowthInside1LastX128,
                uint128 tokensOwed0,
                uint128 tokensOwed1
            );

        function balanceOf(address owner) external view returns (uint256);

        function tokenOfOwnerByIndex(address owner, uint256 index)
            external
            view
            returns (uint256);
    }
}

sol! {
    /// Dipancarkan saat likuiditas ditambahkan, baik lewat `mint` maupun
    /// `increaseLiquidity`.
    ///
    /// Dipakai `cargo xtask golden` untuk menemukan transaksi sungguhan yang
    /// bisa dijadikan golden vector. Tanpa indexer, log adalah satu satunya
    /// cara menemukan transaksi berdasarkan kontrak tujuan.
    #[derive(Debug)]
    event IncreaseLiquidity(
        uint256 indexed tokenId,
        uint128 liquidity,
        uint256 amount0,
        uint256 amount1
    );
}

sol! {
    #[sol(rpc)]
    interface IERC20Write {
        function approve(address spender, uint256 amount) external returns (bool);
        function allowance(address owner, address spender) external view returns (uint256);
        function balanceOf(address account) external view returns (uint256);
    }
}
