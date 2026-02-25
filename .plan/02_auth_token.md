# 02 Auth: GetUserAuth

## Goal
`GetUserAuth` でトークン取得し、他コマンドで再利用できるようにする。

## Owned Files
- `src/reolink/auth.rs`
- `src/app/usecases/get_user_auth.rs` (新規)
- `src/app/usecases/mod.rs`
- `src/interfaces/cli/args.rs` (authサブコマンド追加)
- `src/interfaces/cli/handlers.rs` (authハンドリング)
- `src/interfaces/mcp/tools.rs` / `src/interfaces/mcp/handlers.rs` (`reolink.get_user_auth` 追加)
- `tests/auth_integration.rs` (新規)

## Tasks
1. `GetUserAuth` 実装
- 入力: `user/password`
- 出力: `token`（レスポンスJSONから抽出）
- 失敗時: `rspCode` を含むメッセージで返却

2. トークン利用導線
- CLIで `auth login` 実行時にトークンを表示。
- MCPでトークン返却。
- 以後の実行で `Auth::Token` を使える入口を用意（保存方式は次段で可）。

3. 認証エラー切り分け
- `please login first` / `401` を見分ける。

## Done Criteria
- 実機で `GetUserAuth` が成功し、トークン文字列を確認できる。
- 認証失敗時に原因が判断しやすいエラーメッセージになる。

## Dependencies
- `01_foundation.md`
