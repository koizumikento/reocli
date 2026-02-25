# 03 Ability: GetAbility + Capability Gate

## Goal
`GetAbility` をパースし、コマンド実行可否を事前判定できるようにする。

## Owned Files
- `src/reolink/device.rs`
- `src/core/model.rs` (`Ability` 拡張)
- `src/app/usecases/get_ability.rs`
- `src/app/preflight.rs`
- `tests/ability_integration.rs` (新規)

## Tasks
1. `GetAbility` 実レスポンスパース
- `supported_commands` を静的配列ではなく実レスポンス由来に置換。

2. 実行可否ヘルパー
- `supports(cmd)` などを `Ability` に追加。
- 未対応時は `UnsupportedCommand` を返す。

3. Preflight強化
- `preflight` で「機種/FW/対応cmd数」を表示。

## Done Criteria
- 実機で `GetAbility` の結果が固定値ではなく変動する。
- `GetAbility` で未対応と判定された `cmd` を実行すると、事前に失敗できる。

## Dependencies
- `01_foundation.md`
