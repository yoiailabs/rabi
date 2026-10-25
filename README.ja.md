# rabi

rabi（ラビ、rabbit から） — コーディングエージェントの並列作業用の独立した Git ワークスペース。

> [!WARNING]
> rabi は活発に開発を進めている v7 ベータ版です。v7 のベータ期間中は、SemVer の 0.x と同じルールで、マイナーリリース（7.x）に破壊的変更が含まれることがあります。パッチリリース（7.x.y）は不具合修正のみです。破壊的変更は [CHANGELOG](CHANGELOG.md) に記載しています。

タスクごとに独立した Git ワークスペース（リンクされた worktree）を用意し、複数のエージェントと人間が同じリポジトリで並行して作業できます。rabi はチェックアウトの目的を記録し、機械可読の出力を提供し、削除前に履歴の保全を確認します。未保全のコミットや未コミットの変更を破棄するには、明示的な許可が必要です。

[English](README.md) · [コマンドリファレンス](docs/commands.md) · [JSON 仕様](docs/json.md)（詳細ドキュメントは英語）

## インストール

最新の v7 リリースをインストールします。インストーラーは常に SHA-256 を検証し、`curl` と `tar` を必要とします。

```sh
curl -fsSL https://github.com/yoiailabs/rabi/releases/latest/download/install.sh | sh
```

`sh -s -- --verify-attestation` を使うと、配布物の来歴も検証できます（`gh` が必要です）。

Linux・macOS の x64/arm64 に対応しています。WSL では Linux バイナリを使ってください。sudo やシェル設定の変更なしで `~/.local/bin` にインストールします。

ソースからのビルドには Rust 1.89 以降が必要です。

```sh
cargo install --git https://github.com/yoiailabs/rabi --tag v7.0.0 --locked rabi
```

npm の `@phantompane/cli` と Homebrew の `phantom` は旧 TypeScript 版 v6 です。

エージェントや CI では、`RABI_VERSION=<TAG>` と同じタグのインストーラー URL でバージョンを固定できます。バージョン固定、旧リリース、手動検証、オプション、更新方法は[インストールの詳細](docs/installation.md)を参照してください。

## クイックスタート

既存の clone と `main` ブランチを使います。必要に応じてベースブランチを置き換えてください。`sync` には対象リモートへのアクセスが必要です。この例ではコードを変更しないため、保全済みのタスクブランチを削除できます。フックのあるリポジトリでは[信頼の付与または明示的なスキップ](docs/hooks.md)が必要です。

```sh
cd your-repo
rabi create task/auth --base main --purpose 'implement authentication'
rabi exec task/auth -- git status --short
rabi list
rabi sync task/auth
rabi remove task/auth --dry-run
rabi remove task/auth
```

`exec` はシェルの現在地を変えずにワークスペースで実行します。自分で移動する場合は、削除前に `cd "$(rabi path task/auth)"` を使うか、任意の[シェル関数と補完](docs/shell.md)を読み込んで `rabi cd task/auth` を使ってください。

## 基本概念

- **ワークスペース:** タスクのチェックアウトと、目的・ベース・PR/issue リンクなどのメタデータです。名前に `/` を使えます。名前、`.`、パス、`pr:N`、`issue:N`、記録済みの PR/issue URL で選択します。
- **プライマリチェックアウト:** リポジトリの元のチェックアウトで、ファイルのコピー元です。rabi はこれを削除しません。デフォルトブランチ上なら `sync --main` で明示的に更新できます。
- **記録と管理:** 記録は Git 共通ディレクトリに保存します。未記録のチェックアウトも表示されます。`adopt` は既存のリンクされたチェックアウトを記録し、`forget` は記録だけを削除します。設定されたワークスペースディレクトリの外にある未記録のチェックアウトは管理対象外で、削除から保護されます。
- **履歴の保全:** ブランチは保全を証明できる場合に削除し、証明できない場合は警告とともに残します。未コミットの変更の破棄には `--discard-changes`、未保全の履歴の削除には `--delete-unpreserved-branch` が必要です。無視ファイルはワークスペースと一緒に削除され、無視ディレクトリ内の入れ子のリポジトリも含みます。`gc` は `--apply` まで計画だけを表示し、安全に削除できない対象をスキップします。

自動化の前に[削除の安全性と再試行](docs/safety.md)、[同期戦略](docs/sync.md)を確認してください。所有者管理、内部ロック、使用中かどうかの検出はないため、他のエージェントと調整してください。

## コマンド

| コマンド | 用途 |
|---|---|
| `rabi create` | 新規・既存ブランチ、PR、issue、切り離したコミットから作成 |
| `rabi adopt` | 既存のリンクされたチェックアウトを記録 |
| `rabi list` | ワークスペースとプライマリチェックアウトの一覧 |
| `rabi show` | 目的と現在の状態を確認 |
| `rabi path` | ワークスペースの絶対パスを出力 |
| `rabi set` | 目的、ベース、PR・issue のリンクを更新 |
| `rabi setup` | 無視ファイルをコピーし、信頼済みのセットアップフックを再実行 |
| `rabi exec` | ワークスペースでコマンドを実行（`--` が必須） |
| `rabi sync` | fetch してベースを取り込む。`--main` でプライマリも更新 |
| `rabi remove` | 指定したワークスペースを安全性を確認して削除 |
| `rabi forget` | ファイルとブランチを残して記録だけを削除 |
| `rabi gc` | クリーンアップを計画し、`--apply` で実行 |
| `rabi clone` | ghq 互換の配置に clone して登録 |
| `rabi repo` | リポジトリ登録の操作 |
| `rabi repo add` | リポジトリを登録 |
| `rabi repo list` | 登録済み・検出されたリポジトリの一覧 |
| `rabi repo show` | リポジトリを確認 |
| `rabi repo forget` | 登録だけを削除 |
| `rabi run` | ワークスペースの親セッションを起動・再開 |
| `rabi skill` | スキル探索 |
| `rabi skill list` | スキル名・探索元・パスを一覧表示 |
| `rabi trust` | フック・run 実行の信頼設定 |
| `rabi trust add` | フックを表示し、確認プロンプトなしで信頼を付与 |
| `rabi trust remove` | 信頼を取り消す |
| `rabi trust list` | 信頼済みの Git 共通ディレクトリの一覧 |
| `rabi config` | ユーザー設定の操作 |
| `rabi config get` | 有効な設定値を取得 |
| `rabi config set` | 設定値を保存 |
| `rabi config unset` | 設定の上書きを解除 |
| `rabi config list` | 有効な設定値の一覧（`--sources` で設定元を表示） |
| `rabi help` | テキストヘルプ、または完全な JSON コマンド仕様 |
| `rabi completions` | bash・zsh・fish の補完スクリプトを生成 |
| `rabi shell-init` | `rabi cd` 用のシェル関数を生成 |
| `rabi --version` | アプリのバージョンを表示（`-v` も利用可） |

グローバルオプションは `-R/--repo REF`、`--json`、`--output text|json`、`--no-input`、`-h/--help`、`-v/--version` です。`run` と `skill` は現在のチェックアウトを使い、`-R` を受け付けません。`exec` では rabi のオプションをセレクターの前に、子コマンドを `--` の後に置きます。[全オプションと使用例](docs/commands.md)を参照してください。

## エージェント向け

`--json` または `--output json` で、`schema`、`command`、`ok`、`data`、`warnings`、`error` を持つ schema 2 のエンベロープを 1 つ出力します。失敗時も `data` に部分的な結果が入ることがあるため解析し、未知のフィールドは無視してください。`rabi help --json` でコマンド、フラグ、エラー・警告コードを取得できます。

`--no-input` は rabi の確認プロンプトを無効にします。JSON 出力、空でない `CI`、空でも `0` でもない `RABI_NO_INPUT`、標準入力または標準エラーが端末でない場合も同様です。未信頼のフックは変更前に拒否され、`gc` はその対象をスキップします。単独の `exec` は子コマンドの出力と終了ステータスを保持します。`exec`、`completions`、`shell-init` は明示的な JSON 指定を拒否します。

| 終了コード | 意味 |
|---|---|
| 0 | 成功 |
| 1 | 実行上の失敗 |
| 2 | 使用方法の誤り、または曖昧なセレクター |
| 3 | 対象またはリポジトリが見つからない |
| 4 | 安全上の拒否 |
| 5 | 同期の競合 |
| 6 | 永続的な変更後の部分的な完了 |
| 124 | run の制限時間超過 |
| 130 | run の中断 |

[JSON オブジェクトとコマンド別データ](docs/json.md)、[終了コードと復旧](docs/exit-codes.md)を参照してください。

## エージェントスキル

Node.js/npm を使って、Codex、Claude Code、Cursor 向けに [rabi の使い方を学ぶスキル](skills/rabi/SKILL.md)をインストールできます。

```sh
npx skills add yoiailabs/rabi --skill rabi -a codex -a claude-code -a cursor -g -y
npx skills update rabi
```

`-g` はグローバルにインストールします。更新コマンドにも `-g` を付けると、グローバルを明示的に指定できます。対応するすべてのエージェントにインストールするには、個別のエージェント指定の代わりに `-a '*'` を使ってください。このリポジトリには `.codex/skills` 配下にチェックアウト内の開発者向けスキルもあるため、`--skill rabi` が必要です。スキルは CLI の使い方を説明するものです。rabi バイナリは上記の手順で別途インストールしてください。

## 親セッションとスキル

`rabi run --skill rabi-dev "Fix issue #123"` は現在のワークスペースで Codex の親セッションを1つ起動します。親が進行管理、subagent への委任、証拠収集、完了判断を担当します。次の `rabi run` は同じ親を再開し、`--fresh --skill VALUE` は古い記録を残して新しく開始します。`--skill` はファイルを解決せず値をそのまま指示に渡します。`--timeout 30m` で起動ごとの制限時間を変更できます（既定 `2h`）。リポジトリの信頼が必要です。先にワークスペースを作成してください。[コマンド](docs/commands.md)を参照。

`rabi skill list` はチェックアウトとユーザーのスキルディレクトリから直下のスキルを重複も含めて一覧表示します。`rabi run` はこの探索を使いません。

```sh
rabi skill list
rabi run --skill rabi-dev "Fix issue #123"
rabi run "Address the review findings"
rabi run --fresh --skill rabi-dev - < task.md
```

## 設定

`rabi config` で `workspace-root`、`clone-root`、`discover-ghq`、`sync.strategy`、`sync.rewrite-published` を管理します。`-R/--repo REF` はパス、登録済みの名前、検出された名前でリポジトリを選びます。[設定・パス・リポジトリ登録](docs/configuration.md)を参照してください。

## フックと信頼

リポジトリの `.rabi.toml` に setup・pre-remove フックを定義します。`.worktreeinclude` はプライマリからコピーする無視ファイルを選びます。`rabi trust add` で信頼を付与する前にフックと親への指示を確認してください。信頼は Git 共通ディレクトリのパスに付与され、同じパスに clone し直しても取り消すまで有効です。[フック・信頼・セットアップの再試行](docs/hooks.md)を参照してください。

## FAQ

**普通の `git worktree` との違いは？** Git がリンクされたチェックアウトを提供し、rabi はタスクのメタデータ、リポジトリ検出、準備、ベースとの同期、根拠に基づくクリーンアップを追加します。ブランチと worktree は通常の Git のままです。

**エージェントの実行やサービスの分離も行う？** ワークスペースが分けるのはファイルとブランチです。rabi はエージェントのスケジュール、端末セッションの管理、プロセス・ポート・データベース・認証情報・エージェント間アクセスの分離は行いません。

## ステータス

ベータ版です（上のバージョニング方針を参照）。破壊的変更は [CHANGELOG](CHANGELOG.md) に記載しています。

## コントリビューション

開発手順とチェックは [CONTRIBUTING.md](CONTRIBUTING.md) を参照してください。プルリクエストの宛先は `main` です。

## ライセンス

[MIT](LICENSE)。
