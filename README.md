# BridgingMod (Horizon WebAssembly Mod)

Reach-around block placement ("bridge assist") for Horizon (`api = "0.2"`), running sandboxed as `mod.wasm`.

**No native binary, native-code trust prompt, or Rust toolchain required on the player's machine.**

## Credits & Attribution

- **Original creator:** Will Scully (`squeeglii`)
- **Original project:** [squeeglii/BridgingMod](https://github.com/squeeglii/BridgingMod)
- **Horizon native port:** [xiryuudev/bridgingMod-horizon](https://github.com/xiryuudev/bridgingMod-horizon)
- **WASM port and maintainer:** iuaiti
- **WASM repository:** [iuaiti/bridgingmod-horizon-wasm](https://github.com/iuaiti/bridgingmod-horizon-wasm)

This project adapts the Horizon native port to run entirely inside Horizon's WebAssembly sandbox.

## Features

- **Reach-around placement:** Right-click with a block while looking into a gap next to an edge to place a block without aiming at a block face.
- **Crosshair direction chevron:** Shows which direction the next bridged block will go.
- **Slab & trapdoor assist:** Additional placement support for slabs and trapdoors.
- **Crouch rules:** Configure bridging behavior while crouching.
- **Missed-corner adjacency:** Configure how nearby block corners and edges are considered when finding a placement target.
- **Toggle key:** Press `,` by default to enable or disable bridging assist. The key can be configured in Controls.

Settings are available under **Options > Mods > BridgingMod**.

## WebAssembly

- Runs sandboxed as `mod.wasm`.
- Does not require native compilation on the player's machine.
- Does not require a native-code trust prompt.
- Uses Horizon Mod API `0.2`.
- Distributed as a `.hmod` package.

## Build (WebAssembly)

Requires Rust (`rustup`, Rust 1.95+) and `cargo-horizon-mod` for development and building.

Install the build tool:

```bash
cargo install --locked --git https://github.com/horizon-rust/horizon cargo-horizon-mod
```

Optionally update the SDK pin in `Cargo.toml` (`[package.metadata.horizon] sdk`) to match the latest Horizon Nightly:

```bash
cargo horizon-mod update
```

Build `mod.wasm` and package the mod:

```bash
cargo horizon-mod build --wasm
```

Expected output:

```text
target/horizon-mod/bridgingmod_horizon.hmod
```

Do not edit the `[wasm]` section in `horizon-mod.toml` manually. The build command generates `mod.wasm` and the `[wasm]` configuration automatically.

**Note:** Developers need Rust and Cargo to build the mod. Players only need the packaged `.hmod` file.

## Installation

1. Copy `target/horizon-mod/bridgingmod_horizon.hmod` into your game's `horizon-mods/` directory.
2. Add the following line to `horizon-mods.txt` beside `options.txt`:

   ```text
   mod.bridgingmod_horizon:true
   ```

3. Launch Horizon.
4. Configure the mod through **Options > Mods > BridgingMod**.

## License

MIT. See [LICENSE](LICENSE).

This project retains attribution to the original BridgingMod creator and the Horizon native port author. Please preserve the applicable upstream license notices when redistributing.
