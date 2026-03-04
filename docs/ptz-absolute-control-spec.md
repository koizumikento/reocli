# PTZ 絶対位置制御（Raw Count）仕様

最終更新: 2026-03-03

## 1. スコープ

- 対象軸: `Pan/Tilt`（`Zoom` は対象外）
- 座標: `GetPtzCurPos` の生カウント値（角度換算しない）
- 実装対象: CLI / MCP の `set-absolute` と `get-absolute`
- 制御方式: 状態フィードバックによる閉ループ制御

## 2. 公開 I/F（実装済み）

### 2.1 CLI

- `reocli ptz set-absolute <pan_count> <tilt_count> [--tol-count <i64>] [--timeout-ms <u64>] [--channel <0-255>]`
- `reocli ptz get-absolute [--channel <0-255>]`

既定値:

- `channel`: `0`
- `tol-count`: `10`
- `timeout-ms`: `25000`

入力バリデーション:

- `tol_count > 0`
- `timeout_ms > 0`

### 2.2 MCP

- `reolink.ptz_set_absolute [channel?] <pan_count> <tilt_count> [tol_count] [timeout_ms]`
- `reolink.ptz_get_absolute [channel?]`

## 3. バックエンドとトランスポート

- `REOCLI_PTZ_BACKEND`
  - `cgi`（既定）
  - `onvif`（ContinuousMove/RelativeMove を使用）

`set-absolute` は基本的に `ptz_transport` を通じて PTZ 移動/停止を実行する。

### 3.1 ONVIF 時の条件付き CGI フォールバック

`REOCLI_PTZ_BACKEND=onvif` でも、以下をすべて満たす場合はパルス移動と停止のみ CGI を強制する。

- `supports_relative_pan_tilt_translation == false`
- `has_timeout_range == true`
- `timeout_min` が `PT{秒}S` 形式で解釈可能
- `timeout_min >= PT1S`（1000ms 以上）

このフォールバック時は、完了判定で ONVIF の `motion_status_hint` を使わない。

## 4. 制御ループ概要（`ptz_set_absolute_raw`）

1. 入力検証後、現状態とレンジ（`GetPtzCurPos` / `GetPtzStatus`）を取得
2. 保存済みキャリブレーションと EKF 状態を読み込み
3. 各ステップで観測値と推定値を更新し、制御誤差を算出
4. 目標近傍かつ RelativeMove 対応時は ONVIF RelativeMove を優先
5. それ以外は方向・速度・パルス幅を決めて移動（`move_ptz`）
6. 許容誤差、安定ステップ数、バックエンド完了ヒントを用いて収束判定
7. タイムアウト時はベスト観測値のラッチ判定を試行し、失敗時は詳細付きエラーを返す
8. タイムアウトエラー時は 1 回だけ再試行（`timeout_ms * 3` を `18000..36000ms` にクランプ）
9. 正常/異常どちらでも最後に best-effort で停止コマンドを送る

## 5. ONVIF オプション確認方法

ONVIF backend 利用時は以下で PTZ 機能を確認できる。

```bash
REOCLI_PTZ_BACKEND=onvif reocli ptz onvif options --channel 0
```

出力項目（抜粋）:

- `supports_relative_pan_tilt_translation`
- `has_timeout_range`
- `timeout_min`
- `timeout_max`

`set-absolute` のトランスポート判定は上記値に依存する。

## 6. 実行例

```bash
# CGI backend (default)
reocli ptz set-absolute 1500 -180 --tol-count 12 --timeout-ms 25000 --channel 0

# ONVIF backend
REOCLI_PTZ_BACKEND=onvif reocli ptz set-absolute 1500 -180 --tol-count 12 --timeout-ms 25000 --channel 0

# MCP
reocli-mcp reolink.ptz_set_absolute 0 1500 -180 12 25000
reocli-mcp reolink.ptz_get_absolute 0
```

## 7. 注意点

- 本仕様は角度ではなく raw count を扱う。
- 機種/FW 差により `onvif options` の値は変動する。
- RelativeMove 非対応かつ `timeout_min` が大きい機種では、ONVIF backend 指定中でも一部コマンドが CGI 経由になる。
