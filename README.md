# dragonwilds-trainer

An open source trainer for **RuneScape: Dragonwilds**, written in Rust.

> Status: early development. Tested on game version 1.0.2.2 (Steam build
> 26658295, Unreal Engine 5.6.1), solo world, Windows.

## Scope

For your own solo world, or your own dedicated server with everyone's agreement.
Never use it on someone else's server.

| Feature | Default | Verified in game |
| --- | --- | --- |
| Free runes (rune pouch kept full) | on | yes |
| No spell cooldown | on | yes |
| Full slots (backpack 37-39, first ammo slot) | on | yes |
| Infinite health | **off** | not yet |
| Zero weight | on | yes |

In game, press `Esc`: an icon shows in the top-right corner. Click it to open the
panel and switch each feature on or off. Choices are saved in
`dw-trainer/settings.json`.

## Install

1. Build `version.dll` (see below). There are no prebuilt releases yet.
2. Copy it to `RSDragonwilds/Binaries/Win64/` inside the game folder.
3. Start the game from Steam as usual.

To remove it, delete `version.dll` and the `dw-trainer` folder. The trainer
writes its log to `dw-trainer/trainer.log`. With an empty file named
`READONLY` in `dw-trainer/`, it only reads the game and logs what it finds,
without changing anything.

Requires the game's default DirectX 12 renderer.

## How it works

The trainer finds every game function and field **by name** through Unreal
Engine's own reflection data each time the game starts, instead of using fixed
addresses. Game updates usually move code around but rarely rename things, so
most patches should not break it.

| Crate | Role |
| --- | --- |
| `dw-core` | Platform-independent: UE 5.6 reflection reader and the rules of each feature. Fully testable without the game. |
| `dw-win` | Windows DLL loaded by the game as `version.dll`: finds the engine, hooks `UObject::ProcessEvent` for a game-thread tick, applies the features and draws the panel ([hudhook](https://github.com/veeenu/hudhook)). |
| `dw-probe` | Developer tool: inspects a running game from outside, read-only (`dw-probe <objects> <names> player`; the two offsets are in `trainer.log`). |
| `dw-linux` | `LD_PRELOAD` library for a Linux dedicated server, with a per-player allowlist. *(planned)* |

## Build and test

```sh
cargo test --workspace
cargo build -p dw-win --release   # target/release/version.dll
```

## Notes

- The game ships a crash reporter. If the game crashes while the trainer is
  loaded, the report sent to the developer lists the trainer's DLL.
- Not affiliated with or endorsed by Jagex.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
