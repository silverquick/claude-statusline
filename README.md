# StatusLinePowerline

`StatusLinePowerline` は、Claude Code から標準入力で渡される JSON を読み、ANSI カラー付きの Powerline 形式ステータス行を標準出力へ返す Rust 製のコンソールアプリケーションです。

通常の `statusLine` では、次の情報を左から順に表示します。

1. `ユーザー名@ホスト名`
2. 作業ディレクトリ
3. モデル
4. Effort
5. コンテキスト使用量（トークン数・使用率バー・パーセント）
6. 5時間・7日間の利用率（使用率バー・パーセント・リセット時刻）
7. Gitブランチ、ワークツリー、差分行数

先頭2つはシェルの `PS1` に使われる `\u@\h:\w` と同じ並びで、そのあとにセッション情報、利用率、Git情報と続きます。項目名（`Model:`、`Cwd:` など）はセグメントの色と位置で区別できるため表示しません。時間窓を区別する必要がある `5h` / `7d` と、ブランチと紛らわしい `WT:` のみラベルを残しています。

末尾の `(+154,-1163)` は `git diff --numstat HEAD --` の合計、つまり `HEAD` と比べた作業ツリーの増減行数です。ステージ済みと未ステージの両方を含み、未追跡ファイルは含みません。

`--subagent` を付けると `subagentStatusLine` 用に動作し、サブエージェントごとの表示内容を JSON Lines で返します。

> [!IMPORTANT]
> ソースはクロスプラットフォームな標準ライブラリのみで書かれていますが、1つの実行ファイルをすべてのOSで共用することはできません。Windows、Linux、macOSそれぞれのOS・CPUに合うターゲットでビルドしてください。

## 必要条件

### ビルドするPC

- Rust ツールチェーン（`cargo` を含む）

`Cargo.toml` は `rust-version = "1.74"` を宣言しています。動作検証は Rust 1.97.1 で行いました。

依存クレートは2つだけです。

| クレート | 用途 |
| --- | --- |
| `serde_json` | 標準入力のJSONと `~/.claude.json` の解析 |
| `chrono` | 利用率リセット時刻のローカルタイムゾーン変換と書式化 |

### 実行するPC

- Claude Code
- UTF-8とANSI truecolorを扱えるターミナル
- PowerlineまたはNerd Font対応フォント
- 差分行数を表示する場合は、`PATH` 上の `git`

**別途ランタイムをインストールする必要はありません。** ビルド成果物は単体で動作するネイティブバイナリです（Linux では `libc` と `libgcc` のみに依存）。

Git実行ファイルは必須ではありません。ブランチとワークツリーは `.git` メタデータから直接検出できます。`git` が必要なのは差分統計の取得だけで、取得できない情報は表示から省略されます。

## 対象ターゲット

`Cargo.toml` に固定のターゲットはありません。実行先と同じOS・CPU上でビルドするのが最も簡単です。

| 実行先 | ターゲットトリプル | 成果物名 |
| --- | --- | --- |
| Linux x64 | `x86_64-unknown-linux-gnu` | `StatusLinePowerline` |
| Linux Arm64 | `aarch64-unknown-linux-gnu` | `StatusLinePowerline` |
| macOS Intel | `x86_64-apple-darwin` | `StatusLinePowerline` |
| macOS Apple Silicon | `aarch64-apple-darwin` | `StatusLinePowerline` |
| Windows x64 | `x86_64-pc-windows-msvc` | `StatusLinePowerline.exe` |
| Windows Arm64 | `aarch64-pc-windows-msvc` | `StatusLinePowerline.exe` |

Linuxの `*-linux-gnu` は標準的なglibc環境を対象としています。Alpine Linuxなどのmusl環境は `*-unknown-linux-musl` ターゲットによる別ビルドと実機検証が必要です。

macOSではIntel Macに `x86_64-apple-darwin`、Apple Siliconに `aarch64-apple-darwin` を使用します。このリポジトリは署名・notarization済みバイナリの配布を前提としていません。

### クロスコンパイルについて

別OS・別CPU向けのビルドには、`rustup target add <target>` に加えて**そのターゲット用のリンカ**が必要です。

| 元 → 先 | 追加で必要なもの |
| --- | --- |
| Linux → Linux Arm64 | `gcc-aarch64-linux-gnu`（+ `.cargo/config.toml` でリンカ指定） |
| Linux → Windows | `mingw-w64` と `x86_64-pc-windows-gnu` ターゲット |
| Linux → macOS | Apple SDKが必要なため実質困難。macOSランナー上でのビルドを推奨 |

実行先と同じOS上でビルドできるなら、そちらのほうが確実です。

## ソース、生成物、設定の役割

ソースの場所、実行ファイルのインストール先、Claude Codeの設定は別々に扱います。

| 対象 | 役割 | 推奨例 |
| --- | --- | --- |
| ソースディレクトリ | `src/main.rs` と `Cargo.toml` を編集・ビルドする場所 | 任意のチェックアウト先 |
| `target/` | `cargo build` が作る生成物 | インストール先には使わない |
| インストール先 | Claude Codeが実際に起動する固定パス | `$HOME/.local/bin/StatusLinePowerline` |
| Claude Code設定 | 実行するstatuslineコマンドを指定する | `~/.claude/settings.json` |
| 別のユーザー設定ディレクトリ | Claudexなどの分離構成 | 例: `~/.claude-sol/settings.json` |

処理の流れは次のとおりです。

```text
ソース
  └─ cargo install --path . --root <prefix>
       └─ <prefix>/bin/StatusLinePowerline
            └─ settings.json の command が起動
                 └─ stdin JSON → StatusLinePowerline → stdout
```

`cargo build` で `target/release/` が更新されても、Claude Codeの設定が別のインストール先を参照していれば表示は更新されません。ソースを変更したら、実際に設定している場所へ再インストールしてください。

## ビルドとインストール

以下のコマンドは、ソースディレクトリで実行します。

### 開発用ビルド

```sh
cargo build --release
```

成果物は `target/release/StatusLinePowerline`（Windowsでは `.exe`）です。この場所のまま設定から参照することもできますが、`cargo clean` で消えるため恒久的なインストール先には向きません。

### インストール

`cargo install` は、ビルドと固定パスへの配置をまとめて行います。`--root <prefix>` を指定すると `<prefix>/bin/` へ配置されます。

```sh
cargo install --path . --root "$HOME/.local" --locked
```

成果物:

```text
$HOME/.local/bin/StatusLinePowerline
```

`--locked` は `Cargo.lock` に記録されたバージョンをそのまま使う指定です。実行権限は `cargo install` が付与するため、`chmod` は不要です。

ソースを変更して入れ直す場合も、同じコマンドを再実行します（`--force` は不要です）。

### 手動配置

インストール先を細かく制御したい場合は、ビルドしてから任意の場所へコピーします。

```sh
cargo build --release
install -Dm755 target/release/StatusLinePowerline "$HOME/.local/bin/claude-statusline/StatusLinePowerline"
```

### クロスコンパイルする場合

```sh
rustup target add aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu
```

成果物は `target/<target>/release/StatusLinePowerline` に出ます。前述のとおり、ターゲット用リンカが別途必要です。

## Claude Codeの設定

`statusLine` と `subagentStatusLine` は任意の設定スコープで指定できます。本書では個人用設定として、通常のユーザー設定ファイルを使用します。

- Windows: `%USERPROFILE%\.claude\settings.json`
- Linux/macOS: `~/.claude/settings.json`

既存の `settings.json` 全体を置き換えず、次の2キーを既存のJSONオブジェクトへ追加または更新してください。

- `statusLine`: 通常表示
- `subagentStatusLine`: 同じ実行ファイルを `--subagent` 付きで実行

両方とも、最低限次の形式を持つオブジェクトです。

```json
{
  "type": "command",
  "command": "<shell command>"
}
```

`statusLine` では任意で `padding`、`refreshInterval`、`hideVimModeIndicator` も指定できます。`subagentStatusLine` では任意で `padding` を指定できます。

`command` はシェルで実行されます。公式ドキュメントで使われる非引用の `~` はホームディレクトリへ展開され、Windowsでも利用できます。一方、引用符で囲んだ `~` はPOSIXシェルで展開されません。以下では配置先を明確にするため、`<user>` を実際のユーザー名へ置き換える絶対パスを使用します。

### Linux

```json
{
  "statusLine": {
    "type": "command",
    "command": "'/home/<user>/.local/bin/StatusLinePowerline'"
  },
  "subagentStatusLine": {
    "type": "command",
    "command": "'/home/<user>/.local/bin/StatusLinePowerline' --subagent"
  }
}
```

### macOS

```json
{
  "statusLine": {
    "type": "command",
    "command": "'/Users/<user>/.local/bin/StatusLinePowerline'"
  },
  "subagentStatusLine": {
    "type": "command",
    "command": "'/Users/<user>/.local/bin/StatusLinePowerline' --subagent"
  }
}
```

### Windows

Windowsでは、Git BashがインストールされていればstatuslineコマンドはGit Bashで、なければPowerShellで実行されます。Git Bashでは非引用のバックスラッシュがエスケープとして扱われるため、コマンド内のWindowsパスは前方スラッシュで記述します。

PowerShellでは引用したパスだけでは実行されず、呼び出し演算子 `&` が必要です。次の例は、どちらのシェルが選ばれても同じように動作するようPowerShellを明示的に起動します。

```json
{
  "statusLine": {
    "type": "command",
    "command": "powershell -NoProfile -Command \"& 'C:/Users/<user>/.local/bin/StatusLinePowerline.exe'\""
  },
  "subagentStatusLine": {
    "type": "command",
    "command": "powershell -NoProfile -Command \"& 'C:/Users/<user>/.local/bin/StatusLinePowerline.exe' --subagent\""
  }
}
```

## Claudexなどで別の設定ディレクトリを使う場合

`~/.claude-sol` は、Claude Codeが自動的に検索する名前付きプロファイルではありません。Claude Codeを起動する前に `CLAUDE_CONFIG_DIR` を設定すると、既定の `~/.claude` の代わりに指定したディレクトリをユーザー設定ディレクトリとして使えます。

### Windows PowerShell

```powershell
$env:CLAUDE_CONFIG_DIR = "$HOME/.claude-sol"
claude
```

### Linux/macOS

```sh
CLAUDE_CONFIG_DIR="$HOME/.claude-sol" claude
```

この場合は、同じ完全な `statusLine` / `subagentStatusLine` 設定を次へ配置します。

```text
~/.claude-sol/settings.json
```

これはClaudex形式の分離構成の一例であり、すべてのPCで固定されたパスではありません。実際にClaudexが設定している `CLAUDE_CONFIG_DIR` を確認してください。

`CLAUDE_CONFIG_DIR` はユーザー設定、履歴、プラグインなどのユーザー構成を分離します。一方、ワークスペース内の `.claude/settings.json` と `.claude/settings.local.json` はプロジェクトファイルなので、この変数では分離されません。

また、このアプリケーションの利用率フォールバックは、現在 `CLAUDE_CONFIG_DIR` ではなく環境変数 `HOME` 直下にある `~/.claude.json` を読みます。別の設定ディレクトリを使っていても、statusline入力に利用率が含まれていれば通常どおり表示できます。キャッシュを参照するのは、入力に `rate_limits.five_hour` または `rate_limits.seven_day` のオブジェクト自体がない場合だけです。オブジェクトは存在しても `used_percentage` がない場合は、キャッシュへフォールバックせず `--%` になります。

## 入出力契約

### 通常モード

引数なしで起動すると、標準入力を末尾まで読み、JSONオブジェクトからANSI装飾済みのステータス行を1本生成します。

主な入力値:

- モデル: `model.display_name`、なければ `model.id`
- Effort: `effort.level`、文字列の `effort`、`effortLevel`、`reasoningEffort`
- コンテキスト: `context_window.total_input_tokens`、`context_window.context_window_size`、`current_usage`
- 作業ディレクトリ: `workspace.current_dir`、なければ `cwd`
- 利用率: `rate_limits.five_hour`、`rate_limits.seven_day`

`ユーザー名@ホスト名` だけは入力JSONではなく実行環境から取得します。ユーザー名は環境変数 `USER`、`LOGNAME`、`USERNAME` の順に探します。ホスト名は `/proc/sys/kernel/hostname`、`/etc/hostname`、環境変数 `HOSTNAME`、`HOST`、`COMPUTERNAME` の順に探し、`\h` と同じく最初のドットより前だけを使います。どちらか一方しか判定できない場合は、判定できたほうだけを表示します。

> [!NOTE]
> macOS には `/proc` がなく `/etc/hostname` も既定では存在しないため、シェルが `HOSTNAME` または `HOST` をエクスポートしていない場合はホスト名部分が省略され、ユーザー名だけの表示になります。

入力に `rate_limits.five_hour` または `rate_limits.seven_day` のオブジェクト自体がない場合、不足している時間窓ごとに、`~/.claude.json` の `cachedUsageUtilization.utilization` を任意のフォールバックとして読みます。このファイルは必須ではありません。入力オブジェクトは存在しても `used_percentage` がない場合や、不足している時間窓のキャッシュ形式が異なる場合、読み取りに失敗した場合は、その時間窓だけを `--%` と表示します。

Gitリポジトリは作業ディレクトリから親方向へ `.git` を探索します。通常のリポジトリでは `git diff --numstat HEAD --` により、`HEAD` と比較したステージ済み・未ステージの追跡対象変更を集計します。初回コミット前のリポジトリでは `git diff --cached --numstat <empty-tree> --` により、ステージ済み変更だけを空ツリーと比較します。どちらも3秒で打ち切り、未追跡ファイルは差分統計に含めません。

無効なJSONや内部エラーはClaude CodeのUIを妨げないよう静かに処理され、プロセスは終了コード0で終了します。パニックも捕捉されるため、標準エラー出力へ何かが漏れることはありません。

### `--subagent` モード

最初の引数が `--subagent` の場合、入力JSONの `tasks` 配列を処理します。表示可能な各タスクについて、次の形式を1行ずつ出力します。

```json
{"id":"<task id>","content":"<ANSI装飾済みの表示文字列>"}
```

`content` には、存在する場合にモデル、Effort、トークン数またはコンテキスト使用率、説明を含めます。

## スモークテスト

インストール後、Claude Codeを起動する前に直接実行できます。

### Linux/macOS

通常モード:

```sh
binary="$HOME/.local/bin/StatusLinePowerline"
printf '%s\n' '{"model":{"display_name":"Example"},"workspace":{"current_dir":"/tmp/project"},"context_window":{"total_input_tokens":12000,"context_window_size":200000}}' |
  "$binary"
```

サブエージェントモード:

```sh
binary="$HOME/.local/bin/StatusLinePowerline"
printf '%s\n' '{"tasks":[{"id":"task-1","model":"claude-opus-5","effort":"high","tokenCount":12000,"contextWindowSize":200000,"description":"status check"}]}' |
  "$binary" --subagent
```

### Windows PowerShell

通常モード:

```powershell
$binary = "$HOME/.local/bin/StatusLinePowerline.exe"
'{"model":{"display_name":"Example"},"workspace":{"current_dir":"C:\\work"},"context_window":{"total_input_tokens":12000,"context_window_size":200000},"rate_limits":{"five_hour":{"used_percentage":10},"seven_day":{"used_percentage":20}}}' |
  & $binary
```

サブエージェントモード:

```powershell
$binary = "$HOME/.local/bin/StatusLinePowerline.exe"
'{"columns":80,"tasks":[{"id":"task-1","model":"claude-opus-5","effort":"high","tokenCount":12000,"contextWindowSize":200000,"description":"status check"}]}' |
  & $binary --subagent
```

通常モードはANSI装飾済みの1本のstatuslineを返します。`--subagent` は `{ "id", "content" }` のJSONオブジェクトを1行ずつ返します。色やグリフの見え方はターミナルとフォントに依存します。

## トラブルシューティング

| 現象 | 確認・対処 |
| --- | --- |
| `cargo build` 後も表示が古い | `build` は設定が参照するインストール先を更新しません。`cargo install --path . --root <prefix>` で、設定内のパスへ再インストールします。 |
| statuslineが表示されない | 実行ファイルをスモークテストで直接実行し、絶対パスと設定JSONを確認します。 |
| `Permission denied`（Linux/macOS） | `chmod 755` を実行します。ホームが `noexec` の場合は別のユーザー所有ディレクトリへ配置し、設定の絶対パスも更新します。 |
| `Exec format error` | 実行先のOS・CPUと異なるターゲットの成果物です。対象に合うターゲットで再ビルドします。 |
| リンカのエラーでクロスビルドが失敗する | ターゲット用のリンカ（`mingw-w64`、`gcc-aarch64-linux-gnu` など）を導入し、`.cargo/config.toml` で指定します。 |
| Windowsでコマンドを実行できない | Windows設定例どおり前方スラッシュのパスを使い、PowerShellの `&` 呼び出し演算子を含めます。 |
| 色や区切りが崩れる | ANSI truecolor対応端末とPowerline/Nerd Fontを確認します。 |
| モデル、Effort、コンテキストが `?` や `--` になる | Claude Codeから渡されたJSONの該当フィールドがないか、形式が異なります。 |
| 利用率が `--%` になる | 入力の `rate_limits` と、任意の `~/.claude.json` キャッシュを確認します。 |
| Gitブランチやワークツリーが出ない | 作業ディレクトリまたは親に読み取れる `.git` があることを確認します。 |
| Git差分だけが出ない | `git` が `PATH` 上にあること、差分があること、3秒以内に完了することを確認します。未追跡ファイルは対象外です。 |
| `--subagent` が何も返さない | `tasks` が配列で、各対象に空でない `id` と表示可能なモデル・Effort・トークン情報があることを確認します。 |

## 経緯

このアプリケーションは当初 .NET 10 / C# で書かれていました（`Program.cs`、`StatusLinePowerline.csproj`）。実行先に .NET Runtime を要求する点と、statuslineの更新ごとに発生する起動コストを避けるため、外部から見た動作を変えずに Rust へ移植しました。C# 版はgit履歴に残っています。

## 検証状況

`x86_64-unknown-linux-gnu`（Ubuntu 22.04、glibc 2.35、Rust 1.97.1）で次を確認しています。

- `cargo build --release`: 成功、警告0、エラー0
- `cargo clippy --release --all-targets`: 指摘0
- 成果物: 653KB、動的依存は `libc` と `libgcc` のみ
- 起動時間: 約14ms/回（`git diff` 呼び出しを含む実リポジトリ上での50回平均）
- 通常モード: 終了コード0、標準エラーなし
- `ユーザー名@ホスト名`: `/proc/sys/kernel/hostname` から短縮ホスト名を取得できることを確認
- 異常入力（空、非JSON、JSON配列、型不一致）: いずれも終了コード0、標準エラーなし、該当項目のみフォールバック表示
- `rate_limits` 欠落時の `~/.claude.json` フォールバック: 動作を確認
- `used_percentage` 欠落時: `--%` とリセット時刻のみを表示
- `current_usage` の内訳合算: 動作を確認
- Git: 通常リポジトリの差分行数、初回コミット前リポジトリのステージ済み差分、linked worktree（`WT:` セグメントと `commondir` 経由の参照解決）、`workspace.git_worktree` による上書き、detached HEADの8桁表示
- `--subagent` モード: 終了コード0、標準エラーなし、JSON Linesを確認。モデル名整形、日付サフィックス除去、`columns` に応じた説明の切り詰めを確認
- `cargo install --path . --root <prefix> --locked`: 成功、`<prefix>/bin/StatusLinePowerline` を確認

Windows、macOS、Arm64 Linux でのビルドと実行は未検証です。それぞれの実機で別途確認してください。

## 参考資料

- [Customize your status line](https://code.claude.com/docs/en/statusline)
- [Claude Code settings](https://code.claude.com/docs/en/settings)
- [Environment variables](https://code.claude.com/docs/en/env-vars)
