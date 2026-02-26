# 06 PTZ MVP

## Goal
パン・チルト・停止・プリセット移動を安全に使えるMVPを作る。

## Owned Files
- `src/reolink/ptz.rs` (新規)
- `src/core/model.rs` (`PtzDirection`, `PtzSpeed`, `PresetId` 追加)
- `src/app/usecases/ptz_move.rs` (新規)
- `src/app/usecases/ptz_stop.rs` (新規)
- `src/app/usecases/ptz_preset_list.rs` (新規)
- `src/app/usecases/ptz_preset_goto.rs` (新規)
- `src/app/usecases/mod.rs`
- `src/interfaces/cli/args.rs` / `src/interfaces/cli/handlers.rs`
- `src/interfaces/mcp/tools.rs` / `src/interfaces/mcp/handlers.rs`
- `tests/ptz_integration.rs` (新規)

## Tasks
1. コマンド
- `PtzCtrl` で `move(direction, speed)` と `stop`。
- `GetPtzPreset` でプリセット一覧。
- `PtzCtrl(op=ToPos)` でプリセット移動。

2. 安全制御
- `move --duration ms` 指定時は必ず `stop` を送る。
- `speed` 範囲をバリデーション。

3. Abilityガード
- `GetAbility` で未対応なら実行前に拒否。

4. CLI/MCP
- CLI: `ptz move`, `ptz stop`, `ptz preset list`, `ptz preset goto`
- MCP: 同等ツールを追加。

## Done Criteria
- 実機で move/stop が動作。
- プリセット list/goto が動作。
- 不正引数時に安全に失敗。

## Dependencies
- `01_foundation.md`
- `02_auth_token.md`
- `03_ability.md`
