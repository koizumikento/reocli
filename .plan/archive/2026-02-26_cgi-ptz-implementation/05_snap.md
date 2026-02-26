# 05 Snap

## Goal
`Snap` を実データ取得にし、ファイル保存まで完了させる。

## Owned Files
- `src/reolink/media.rs`
- `src/app/usecases/snap.rs`
- `src/interfaces/cli/args.rs` / `src/interfaces/cli/handlers.rs`
- `src/interfaces/mcp/tools.rs` / `src/interfaces/mcp/handlers.rs`
- `tests/snap_integration.rs` (新規)

## Tasks
1. `Snap` 実装方針を決定
- 方式A: `api.cgi?cmd=Snap...` で直接画像バイトを取得。
- 方式B: APIレスポンスからURLを受け、2段階取得。
- まずは実機確認で通る方式に固定。

2. 保存処理
- `--out` 未指定時は `snapshots/channel-<n>.jpg` に保存。
- ディレクトリ自動作成。

3. CLI/MCP
- CLI: `snap [channel] [--out path]`
- MCP: `reolink.snap` は `channel/path/bytes` の最低1つを返す。

## Done Criteria
- 実機でJPEGが保存され、ファイルサイズ > 0。
- 同一チャンネルの連続取得が成功する。

## Dependencies
- `01_foundation.md`
- `02_auth_token.md` (推奨)
- `03_ability.md` (Snap対応判定)
