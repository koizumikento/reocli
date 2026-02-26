# reocli

Rust ベースの Reolink CLI/MCP 実験用プロジェクトです。

## Structure

- `src/core`: ドメイン共通型/エラー/コマンド定義
- `src/reolink`: Reolink API 呼び出し層
- `src/app`: CLI/MCP 共通のユースケース
- `src/interfaces`: CLI と MCP の入出力アダプタ
- `src/bin`: 実行エントリ (`reocli`, `reocli-mcp`)

## Configuration (Environment Variables)

- `REOCLI_ENDPOINT`: API エンドポイント。未設定時は `https://camera.local`。
- `REOCLI_TOKEN`: トークン認証に使用。
- `REOCLI_USER`: ユーザー名。`REOCLI_PASSWORD` が設定されていて未設定/空文字の場合は `admin` を使用。
- `REOCLI_PASSWORD`: パスワード認証に使用。

認証の優先順:

1. `REOCLI_TOKEN`
2. `REOCLI_USER` + `REOCLI_PASSWORD`（`REOCLI_USER` が空なら `admin`）
3. 匿名認証

## Implemented CLI Commands

- `reocli get-user-auth <user> <password>`
- `reocli get-ability [user]`
- `reocli get-dev-info`
- `reocli get-channel-status [channel]`
- `reocli get-ptz-status [channel]`
- `reocli get-time`
- `reocli set-time <iso8601>`
- `reocli snap [channel] [--out path]`
- `reocli ptz move <direction> [--speed <1-64>] [--duration <ms>] [--channel <0-255>]`
- `reocli ptz stop [--channel <0-255>]`
- `reocli ptz preset list [--channel <0-255>]`
- `reocli ptz preset goto <preset_id> [--channel <0-255>]`
- `reocli preflight [user]`

`snap` と `ptz` 系コマンドは実行前に `GetAbility` でサポート確認し、未対応なら `UnsupportedCommand` で失敗します。

## Implemented MCP Tools

- `mcp.list_tools`
- `reolink.get_user_auth`
- `reolink.get_ability`
- `reolink.get_dev_info`
- `reolink.get_channel_status`
- `reolink.get_ptz_status`
- `reolink.get_time`
- `reolink.set_time`
- `reolink.snap`
- `reolink.ptz_move`
- `reolink.ptz_stop`
- `reolink.ptz_preset_list`
- `reolink.ptz_preset_goto`

## Examples

```bash
# Auth
reocli get-user-auth admin secret

# Device / time
reocli get-dev-info
reocli get-time
reocli set-time 2026-02-25T10:00:00Z

# Snap
reocli snap 0 --out snapshots/front-door.jpg

# PTZ
reocli ptz move left --speed 6 --duration 300 --channel 0
reocli ptz stop --channel 0
reocli ptz preset list --channel 0
reocli ptz preset goto 7 --channel 0
```

## Testing

通常テスト（mockito ベース）:

```bash
cargo test --all-targets --all-features
```

実機テスト（`REOCLI_LIVE_TEST=1` のときのみ実行）:

```bash
REOCLI_LIVE_TEST=1 \
REOCLI_ENDPOINT=https://<camera-host> \
REOCLI_TOKEN=<token> \
cargo test --test live_smoke -- --nocapture
```
