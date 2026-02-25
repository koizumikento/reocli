# reocli

Rust ベースの Reolink CLI/MCP 実験用プロジェクトです。

## Structure

- `src/core`: ドメイン共通型/エラー/コマンド定義
- `src/reolink`: Reolink API 呼び出し層
- `src/app`: CLI/MCP 共通のユースケース
- `src/interfaces`: CLI と MCP の入出力アダプタ
- `src/bin`: 実行エントリ (`reocli`, `reocli-mcp`)
