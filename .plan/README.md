# reocli CGI/PTZ Implementation Plan

## Scope
対象は以下です。
- 6機能: `GetUserAuth`, `GetAbility`, `GetDevInfo`, `GetChannelstatus`, `GetTime/SetTime`, `Snap`
- PTZ: `PtzCtrl` を中心に `move/stop/preset` のMVP

## Files
- [01_foundation.md](./01_foundation.md): HTTP/レスポンス基盤
- [02_auth_token.md](./02_auth_token.md): `GetUserAuth`
- [03_ability.md](./03_ability.md): `GetAbility` と実行可否判定
- [04_device_time_channel.md](./04_device_time_channel.md): `GetDevInfo`, `GetChannelstatus`, `GetTime`, `SetTime`
- [05_snap.md](./05_snap.md): `Snap`
- [06_ptz.md](./06_ptz.md): PTZ制御
- [07_test_ci_docs.md](./07_test_ci_docs.md): テスト/CI/ドキュメント

## Parallel Execution Order
Phase 0
- 1) `01_foundation.md`

Phase 1 (並列開始)
- 2) `02_auth_token.md`
- 3) `03_ability.md`
- 4) `04_device_time_channel.md`
- 5) `05_snap.md`

Phase 2 (依存あり)
- 6) `06_ptz.md` (2と3が完了していること)

Phase 3
- 7) `07_test_ci_docs.md`

## Rule For Parallel Work
- 1つのワークストリームは、原則として「自分の担当ファイル範囲」だけを編集する。
- 共有ファイル (`src/interfaces/cli/args.rs`, `src/interfaces/cli/handlers.rs`, `src/interfaces/mcp/handlers.rs`) は、PR順を固定する。
- API挙動差分は必ず `GetAbility` で判定してから実行する。
