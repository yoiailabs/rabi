# rabi

rabi は、ソフトウェア開発の一連の工程を進めるオープンソースの Rust CLI です。`rabi task new` で作業内容を記録し、`rabi task advance` で実装、事前チェック、ドラフト PR の作成、CI、レビュー、リスク判定、承認・マージ・安全なクリーンアップまで進めます。エージェントはローカルで編集・検証・コミットし、公開とマージは明示的な条件を確認する rabi のコマンドが担当します。タスクごとのファイルとブランチは、通常の Git ワークスペースで分離します。

[English](README.md) · [v8 への移行](docs/upgrading-to-v8.md) · [v8.0.0 リリースノート](docs/release-notes/v8.0.0.md)（詳細ドキュメントは英語）

## インストール

[GitHub Releases](https://github.com/yoiailabs/rabi/releases) のインストーラーを使います（`curl` と `tar` が必要です）。

```sh
curl -fsSL https://github.com/yoiailabs/rabi/releases/latest/download/install.sh | sh
```

SHA-256 を検証し、sudo を使わず `~/.local/bin` にインストールします。Linux・macOS の x64/arm64 に対応し、WSL では Linux バイナリを使います。`sh` を `sh -s -- --verify-attestation` に置き換えると、`gh` で配布物の来歴も検証できます。バージョンを固定するには `RABI_VERSION=<TAG>` と同じタグのインストーラー URL を指定します。[インストールの詳細](docs/installation.md)を参照してください。

## クイックスタート

設定の目安は5分です。エージェントの作業と CI には別途時間がかかります。GitHub の既存の clone、ローカルで参照できる `main`、Git 2.44 以降、認証済みの Git と `gh`、インストール・ログイン済みの Codex と Claude CLI が必要です。既定では Codex が実装、Claude がレビューを担当し、リスク判定には Claude のみ対応しています。ベースブランチとチェック内容はプロジェクトに合わせて変更してください。

まず、メンテナーが `.rabi.toml` に事前チェックを追加します。

```toml
[precheck]
commands = ["cargo fmt --check", "cargo clippy --locked --all-targets -- -D warnings", "cargo test --locked"]
```

`.rabi/policy.toml` を追加し、チェック名と app を実際の CI ジョブに、`maintainer-login` を承認権限のあるメンテナーの GitHub ログイン名に置き換えます。

```toml
schema = 1

[ci]
required = ["Rust fmt", "Rust clippy", "Rust test"]
app = "github-actions"

[merge]
auto = ["low", "medium"]

[approval]
approvers = ["maintainer-login"]

[classifier]
doc = "risk-tiers.md"
```

`risk-tiers.md` にレビューとエスカレーションの基準を書きます。low/medium/high を定義し、不確実な場合は high とし、メンテナーの承認が必要な変更を明記してください。タスクを作る**前に**、メンテナーがこの3ファイルをベースブランチへコミットして公開します。事前チェックはリモートのベースコミットと一致する必要があり、ポリシーもベースコミットから読みます。`.rabi.toml`、`.rabi/policy.toml`、`.github/**` の変更は組み込みの保護対象（floor）に該当し、タスクからの公開が停止するため、人間が対応します。

信頼を付与する前にフック・事前チェック・宣言された環境を確認します。プライマリチェックアウトから開始し、クリーンアップでタスクのワークスペースを削除できるよう、シェルはそこに置いたままにします。

```sh
cd your-repo
rabi trust add
rabi task new "Improve the command documentation" --branch task/docs --base main
# Set task_id to the ID printed above.
task_id='task_<uuid>'
rabi create task/docs --base main --no-setup
rabi task implement "$task_id"
rabi task advance "$task_id" --wait
rabi task show "$task_id" --json
```

`task new` は作業内容の記録と未使用ブランチ名の予約だけを行います。`create` でリンクされたチェックアウトを作り、`--no-setup` でコピーとセットアップフックを省略します。最初の実装は `task implement` で開始します。その後は `advance` で1段階ずつ、または `--wait` で進行と待機を繰り返します。流れは、実装 → 事前チェック → 公開（ドラフト PR）→ CI → レビュー → ready → リスク判定 → 承認または自動マージ → クリーンアップです。レビューの指摘で実装に戻る場合も、共通の修正回数制限（既定3回）が適用されます。必須チェックは名前と app ごとに最新の実行を見て、それが成功している必要があります。

`rabi task new --issue N` は、作成者に write・maintain・admin のいずれかの権限があることを確認してから issue を取り込みます。`rabi task intake --label L [--limit N] [--dry-run]` は、大文字と小文字を区別せずにラベルを照合し、最後にそのラベルを付けたユーザーの権限も確認して、既存タスクに紐づく issue をスキップします。ラベルを付けたユーザーを特定できない場合（ボットが付けた場合を含む）は `label_actor_unknown`、権限不足の場合は共同作業者の権限照会で HTTP 404 が返った場合も含めて `intake_not_permitted` を返します。取り込みではエージェントを起動しません。外部の文章は制御文字などを除去したうえで冒頭を残してサイズを制限し、省略時は末尾にマーカーを付けます。後のプロンプトでは信頼できないデータとして毎回異なる nonce で囲みます。その後は上記の手順でチェックアウトを作り、`task implement`、明示的な `task advance` の順に実行します。[外部からの取り込み](docs/commands.md#external-issue-intake)を参照してください。

終了コード10は CI またはメンテナーの承認待ち、4はブロックまたは拒否を表します。`task show --json` の `task.state.reason` とイベントを確認してください。1段階の終了コードが0でも、マージ済みとは限りません。[状態と復旧の表](docs/commands.md#task-lifecycle)、[終了コード](docs/exit-codes.md)を参照してください。

`task implement` は、エージェント終了後の Git メタデータの再検証に失敗した場合も、`stopReason: "metadata_changed"` を記録して終了コード4を返します。再試行する前に Git のワークツリー登録を確認・修復してください。詳しくは[実装の実行結果](docs/json.md#local-task-store)を参照してください。

## 承認

承認なしでマージできるのは、ポリシーで指定した low/medium で floor に該当しない変更だけです。自動マージ対象は既定では空です。high、floor、リスク判定の欠落・失敗・不確実な結果にはメンテナーの承認が必要です。レビュー結果は承認にはなりません。自動化は `rabi task approve` を実行せず、`RABI_AGENT` を解除して回避せず、成功の `rabi/approval` ステータスも投稿しません。

設定されたメンテナーが rabi 起動のエージェントの外で、PR の現在の HEAD と各条件を確認します。`reviewed_sha` に確認した40桁の完全な SHA を設定してから承認を記録します。

```sh
rabi task status task/docs --json
rabi task approve task/docs --sha "$reviewed_sha"
rabi task advance "$task_id" --wait
```

最新の `rabi/approval` が、その SHA に対する `[approval] approvers` の承認者による success である必要があります。HEAD が変われば再承認が必要です。advance が投稿するのは pending だけで、マージ前に全条件を再確認し、`--match-head-commit` を使います。`--wait` の承認待ちは既定で最大24時間で、期限切れは10を返します。クリーンアップは未コミットの変更・stash・未 push のコミットがあるワークスペースと、すべてのローカルブランチを残します。リモートブランチの削除は既定で無効です。[承認とマージの安全性](docs/safety.md#task-approval-and-merge)を参照してください。

## ワークスペース

タスクの工程管理が不要な調査、手動編集、既存ブランチの作業には、ワークスペースのコマンドを使います。プライマリチェックアウトはファイルのコピー元で、削除から保護されます。名前、`.`、パス、記録済みの PR/issue リンクで選択できます。操作対象は現在のリポジトリ、または `-R/--repo` で指定した1つのリポジトリです。チェックアウトのルートパス、または既存の clone 先に対応する正規名 `host/owner/repo` を指定します。相対パスには `./` を付けます。リポジトリの登録と自動検出は廃止されました。

```sh
rabi create topic --base main --purpose 'investigate a change'
rabi exec topic -- git status --short
rabi list
rabi sync topic --dry-run
rabi remove topic --dry-run
```

`sync` は `--dry-run` でも fetch しますが、push はしません。`remove` は履歴の保全を確認し、未保全のブランチを残します。未コミットの変更の破棄と未保全の履歴の削除には、それぞれ明示的な許可が必要です。無視ファイルはワークスペースとともに削除され、無視ディレクトリ内の入れ子のリポジトリも含みます。他のエージェントが使用中かどうかは検出しないため、削除前に調整してください。[ワークスペースのコマンド](docs/commands.md#selecting-and-inspecting-workspaces)、[削除の安全性](docs/safety.md)を参照してください。

## 親セッション

`rabi run` は、既存のリンクされたワークスペースで信頼済みの親セッションを1つ起動・再開します。進行は親自身が管理し、run はタスクの工程を強制せず、完了も判定しません。既定は Codex で、初回または新規開始時に `--provider cursor|claude` で変更できます。`--fresh` は過去の記録を残して新しく開始し、`--timeout` の既定は `2h` です。任意の `--skill VALUE` は検索せず、そのまま指示として渡します。別のワークスペースは `SEL --` で選び、`-R` を使う場合もこれが必要です。

```sh
rabi run topic -- "Investigate the change and commit locally"
rabi run topic --
rabi run --fresh --provider claude --timeout 30m topic -- "Continue the investigation"
```

run にもタスクエージェントと共通の制限が適用されます。継承した GitHub トークン、SSH エージェント・askpass、`GIT_*` を取り除き、gh の設定を空にし、Git の認証情報の取得とプロンプトを無効にします。エージェントは編集・検証・コミットだけを行い、公開処理はその外で実行します。Codex はネットワーク・Web・承認要求を無効にし、Claude は push・gh・Web ツールを禁止します。これは誤った認証情報の利用を減らす措置で、同一ユーザーのプロセスによる意図的なアクセスを防ぐものではありません。[プロバイダーの制限と限界](docs/safety.md#task-agent-launch-boundary)には Cursor の sandbox の動作も記載しています。`list`/`show` は run の概要を表示し、handoff メモリはセッション間で文脈を引き継ぎます。

## コマンド一覧

構文は `rabi help`、全コマンドとフラグの仕様は `rabi help --json` で確認できます。グローバルオプションは `-R/--repo`、`--json`、`--output text|json`、`--no-input`、`-h/--help`、`-v/--version` です。

| 分類 | コマンド |
|---|---|
| タスクの記録 | `rabi task`, `rabi task new`, `rabi task intake`, `rabi task list`, `rabi task show`, `rabi task abandon` |
| 工程の実行 | `rabi task implement`, `rabi task advance`, `rabi task publish`, `rabi task review` |
| 条件の確認 | `rabi task risk`, `rabi task classify`, `rabi task status`, `rabi task approve`, `rabi task merge` |
| ワークスペース | `rabi create`, `rabi list`, `rabi show`, `rabi path`, `rabi set`, `rabi exec` |
| 準備と片付け | `rabi setup`, `rabi adopt`, `rabi forget`, `rabi sync`, `rabi remove`, `rabi gc` |
| セッションとメモリ | `rabi run`, `rabi handoff`, `rabi handoff show`, `rabi handoff path`, `rabi handoff add` |
| 信頼設定 | `rabi trust`, `rabi trust add`, `rabi trust remove`, `rabi trust list` |
| ユーザー設定 | `rabi config`, `rabi config get`, `rabi config set`, `rabi config unset`, `rabi config list` |
| ツール | `rabi clone`, `rabi doctor`, `rabi help`, `rabi completions`, `rabi shell-init`, `rabi --version` |

## ドキュメント

[コマンドとタスクの状態](docs/commands.md) · [設定とポリシー](docs/configuration.md) · [JSON 仕様](docs/json.md) · [終了コード](docs/exit-codes.md) · [安全性](docs/safety.md) · [フックと信頼](docs/hooks.md) · [同期](docs/sync.md) · [シェル連携](docs/shell.md) · [v8 への移行](docs/upgrading-to-v8.md) · [変更履歴](CHANGELOG.md)

## エージェントスキル

[rabi の利用スキル](skills/rabi/SKILL.md)では、タスクの進め方、エージェントのローカル作業、ブロック理由、ワークスペースの復旧方法を説明しています。CLI は上記のリリースインストーラーで別途インストールし、`skills/rabi/` を使っているエージェントのスキル用ディレクトリにコピーしてください。

## コントリビューション

開発と検証は [CONTRIBUTING.md](CONTRIBUTING.md) を参照してください。プルリクエストの宛先は `main` です。

## ライセンス

[MIT](LICENSE)。
