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
