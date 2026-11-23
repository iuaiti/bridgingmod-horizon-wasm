# BridgingMod (Horizon WebAssembly mod)

Reach-around block placement ("bridge assist") for Horizon (`api = "0.2"`), running sandboxed as `mod.wasm` — no native compiler or trust prompt required on the player's machine.

## Features

- **Reach-around placement:** Right-click with a block while looking into a gap next to an edge to place a block without aiming at a block face.
- **Crosshair direction chevron:** Shows which direction the next bridged block will go.
- **Slab & trapdoor assist, crouch rules, missed-corner adjacency, and toggle key** (`,` by default, configurable in Controls and **Options > Mods > BridgingMod**).

## Build (WebAssembly)

Requires Rust (`rustup`, Rust 1.95+) and `cargo-horizon-mod`:

```bash
cargo install --locked --git https://github.com/horizon-rust/horizon cargo-horizon-mod
```

(Optional) Update the SDK pin in `Cargo.toml` (`[package.metadata.horizon] sdk`) to match the latest Horizon Nightly:

```bash
cargo horizon-mod update
```

Build `mod.wasm` and package `target/horizon-mod/bridgingmod_horizon.hmod`:

```bash
cargo horizon-mod build --wasm
```

Do not edit `[wasm]` in `horizon-mod.toml` by hand — `cargo horizon-mod build --wasm` generates `mod.wasm` and `[wasm]` automatically.

## Install

1. Copy `target/horizon-mod/bridgingmod_horizon.hmod` into your game folder's `horizon-mods/` directory.
2. Add `mod.bridgingmod_horizon:true` to `horizon-mods.txt` (beside `options.txt`).
3. Launch Horizon.

## License

MIT. See `LICENSE`.
