# 04 Device/Time/Channel

## Goal
`GetDevInfo`, `GetChannelstatus`, `GetTime`, `SetTime` を実レスポンスベースにする。

## Owned Files
- `src/reolink/device.rs`
- `src/reolink/system.rs`
- `src/app/usecases/get_dev_info.rs`
- `src/app/usecases/get_channel_status.rs` (新規)
- `src/app/usecases/get_time.rs` (新規)
- `src/app/usecases/set_time.rs` (新規)
- `src/app/usecases/mod.rs`
- `src/interfaces/cli/args.rs` / `src/interfaces/cli/handlers.rs`
- `src/interfaces/mcp/tools.rs` / `src/interfaces/mcp/handlers.rs`
- `tests/device_time_channel_integration.rs` (新規)

## Tasks
1. `GetDevInfo`
- モデル/FW/シリアルを実レスポンスから抽出。

2. `GetChannelstatus`
- `channel` 引数で状態を取得。
- `online` 判定をレスポンス由来にする。

3. `GetTime` / `SetTime`
- `GetTime` は装置時刻を返却。
- `SetTime` は入力フォーマット検証 + 実行結果確認。

4. CLI/MCPコマンド追加
- CLI: `channel-status`, `get-time`, `set-time`
- MCP: `reolink.get_channel_status`, `reolink.get_time`, `reolink.set_time`

## Done Criteria
- 各コマンドが固定値ではなく実機レスポンスを返す。
- 異常系（channel不正、時刻形式不正）が再現できる。

## Dependencies
- `01_foundation.md`
- `02_auth_token.md` (実機運用では推奨)
