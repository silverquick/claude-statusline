# StatusLinePowerline

`StatusLinePowerline` は、Claude Code から標準入力で渡される JSON を読み、ANSI カラー付きの Powerline 形式ステータス行を標準出力へ返す .NET コンソールアプリケーションです。

通常の `statusLine` では、次の情報を表示します。

- モデルと Effort
- コンテキスト使用量
- 作業ディレクトリ
- 5時間・7日間の利用率
- Gitブランチ、ワークツリー、差分行数

`--subagent` を付けると `subagentStatusLine` 用に動作し、サブエージェントごとの表示内容を JSON Lines で返します。

> [!IMPORTANT]
> このプロジェクトはソース上ではクロスプラットフォームな .NET API を使用していますが、1つの実行ファイルをすべてのOSで共用することはできません。Windows、Linux、macOSそれぞれのOS・CPUに合うRIDを指定してpublishしてください。

## 必要条件

### ビルドするPC

- .NET 10 SDK

プロジェクトは `net10.0` を対象としています。

### 実行するPC

- 対象OS・CPUに対応する .NET 10 Runtime
- Claude Code
- UTF-8とANSI truecolorを扱えるターミナル
- PowerlineまたはNerd Font対応フォント
- 差分行数を表示する場合は、`PATH` 上の `git`

プロジェクト設定は次のとおりです。

- `PublishSingleFile=true`: アプリケーションを単一ファイルとしてpublishする
- `SelfContained=false`: .NET Runtimeを実行ファイルへ同梱しない

したがって、既定のpublish成果物は**単一ファイルですがフレームワーク依存**です。実行先には .NET 10 Runtimeが必要です。

Git実行ファイルは必須ではありません。ブランチとワークツリーは `.git` メタデータから直接検出できます。`git` が必要なのは差分統計の取得だけで、取得できない情報は表示から省略されます。

## 対象RID

プロジェクトファイルには固定の `RuntimeIdentifier` がありません。publish時に実行先に合うRIDを指定します。

| 実行先 | x64 | Arm64 | 成果物名 |
| --- | --- | --- | --- |
| Windows | `win-x64` | `win-arm64` | `StatusLinePowerline.exe` |
| Linux | `linux-x64` | `linux-arm64` | `StatusLinePowerline` |
| macOS | `osx-x64` | `osx-arm64` | `StatusLinePowerline` |

Linuxの例は標準的なglibc環境を対象としています。Alpine Linuxなどのmusl環境は `linux-musl-*` RIDによる別publishと実機検証が必要です。

macOSではIntel Macに `osx-x64`、Apple Siliconに `osx-arm64` を使用します。このリポジトリは署名・notarization済みバイナリの配布を前提としていません。

## ソース、生成物、設定の役割

ソースの場所、実行ファイルのインストール先、Claude Codeの設定は別々に扱います。

| 対象 | 役割 | 推奨例 |
| --- | --- | --- |
| ソースディレクトリ | `Program.cs` と `.csproj` を編集・ビルドする場所 | 任意のチェックアウト先 |
| `bin/`、`obj/` | `dotnet build` やrestoreが作る一時生成物 | インストール先には使わない |
| リポジトリ内の `publish/` | ローカルpublishで使う場合がある生成物 | 他PC向けの共通バイナリとは扱わない |
| インストール先 | Claude Codeが実際に起動する固定パス | `$HOME/.local/bin/claude-statusline` |
| Claude Code設定 | 実行するstatuslineコマンドを指定する | `~/.claude/settings.json` |
| 別のユーザー設定ディレクトリ | Claudexなどの分離構成 | 例: `~/.claude-sol/settings.json` |

処理の流れは次のとおりです。

```text
ソース
  └─ dotnet publish -r <RID>
       └─ ユーザー別インストール先
            └─ settings.json の command が起動
                 └─ stdin JSON → StatusLinePowerline → stdout
```

`dotnet build` で `bin/Release/...` が更新されても、Claude Codeの設定が別のインストール先を参照していれば表示は更新されません。実際に設定している場所へ `dotnet publish` してください。

## ビルドとpublish

以下のコマンドは、ソースディレクトリで実行します。

### 開発用ビルド

```sh
dotnet build StatusLinePowerline.csproj -c Release
```

`build` は開発用出力を `bin/` に作ります。OS・CPU別に配置する単一ファイルを作る操作は `publish` です。

### publishの基本形

```sh
dotnet publish StatusLinePowerline.csproj -c Release -r <RID> --self-contained false -o <install-directory>
```

Windowsの `.exe` をLinuxやmacOSへコピーしても動作しません。実行先のOS・CPUに合うRIDでpublishしてください。

## OS別のインストール

以下では、管理者権限を必要としないユーザー別ディレクトリへ直接publishします。

### Windows

PowerShellで `win-x64` または `win-arm64` を選択します。

```powershell
$installDir = "$HOME/.local/bin/claude-statusline"
dotnet publish .\StatusLinePowerline.csproj -c Release -r win-x64 --self-contained false -o $installDir
```

成果物:

```text
C:/Users/<user>/.local/bin/claude-statusline/StatusLinePowerline.exe
```

### Linux

`linux-x64` または `linux-arm64` を選択します。

```sh
install_dir="$HOME/.local/bin/claude-statusline"
mkdir -p "$install_dir"
dotnet publish StatusLinePowerline.csproj -c Release -r linux-x64 --self-contained false -o "$install_dir"
chmod 755 "$install_dir/StatusLinePowerline"
```

### macOS

Intel Macでは `osx-x64`、Apple Siliconでは `osx-arm64` を選択します。

```sh
install_dir="$HOME/.local/bin/claude-statusline"
mkdir -p "$install_dir"
dotnet publish StatusLinePowerline.csproj -c Release -r osx-arm64 --self-contained false -o "$install_dir"
chmod 755 "$install_dir/StatusLinePowerline"
```

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

### Windows

Windowsでは、Git BashがインストールされていればstatuslineコマンドはGit Bashで、なければPowerShellで実行されます。Git Bashでは非引用のバックスラッシュがエスケープとして扱われるため、コマンド内のWindowsパスは前方スラッシュで記述します。

PowerShellでは引用したパスだけでは実行されず、呼び出し演算子 `&` が必要です。次の例は、どちらのシェルが選ばれても同じように動作するようPowerShellを明示的に起動します。

```json
{
  "statusLine": {
    "type": "command",
    "command": "powershell -NoProfile -Command \"& 'C:/Users/<user>/.local/bin/claude-statusline/StatusLinePowerline.exe'\""
  },
  "subagentStatusLine": {
    "type": "command",
    "command": "powershell -NoProfile -Command \"& 'C:/Users/<user>/.local/bin/claude-statusline/StatusLinePowerline.exe' --subagent\""
  }
}
```

### Linux

```json
{
  "statusLine": {
    "type": "command",
    "command": "'/home/<user>/.local/bin/claude-statusline/StatusLinePowerline'"
  },
  "subagentStatusLine": {
    "type": "command",
    "command": "'/home/<user>/.local/bin/claude-statusline/StatusLinePowerline' --subagent"
  }
}
```

### macOS

```json
{
  "statusLine": {
    "type": "command",
    "command": "'/Users/<user>/.local/bin/claude-statusline/StatusLinePowerline'"
  },
  "subagentStatusLine": {
    "type": "command",
    "command": "'/Users/<user>/.local/bin/claude-statusline/StatusLinePowerline' --subagent"
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

また、このアプリケーションの利用率フォールバックは、現在 `CLAUDE_CONFIG_DIR` ではなくOSのユーザーホーム直下にある `~/.claude.json` を読みます。別の設定ディレクトリを使っていても、statusline入力に利用率が含まれていれば通常どおり表示できます。キャッシュを参照するのは、入力に `rate_limits.five_hour` または `rate_limits.seven_day` のオブジェクト自体がない場合だけです。オブジェクトは存在しても `used_percentage` がない場合は、キャッシュへフォールバックせず `--%` になります。

## 入出力契約

### 通常モード

引数なしで起動すると、標準入力を末尾まで読み、JSONオブジェクトからANSI装飾済みのステータス行を1本生成します。

主な入力値:

- モデル: `model.display_name`、なければ `model.id`
- Effort: `effort.level`、文字列の `effort`、`effortLevel`、`reasoningEffort`
- コンテキスト: `context_window.total_input_tokens`、`context_window.context_window_size`、`current_usage`
- 作業ディレクトリ: `workspace.current_dir`、なければ `cwd`
- 利用率: `rate_limits.five_hour`、`rate_limits.seven_day`

入力に `rate_limits.five_hour` または `rate_limits.seven_day` のオブジェクト自体がない場合、不足している時間窓ごとに、OSのユーザーホーム直下にある `~/.claude.json` の `cachedUsageUtilization.utilization` を任意のフォールバックとして読みます。このファイルは必須ではありません。入力オブジェクトは存在しても `used_percentage` がない場合や、不足している時間窓のキャッシュ形式が異なる場合、読み取りに失敗した場合は、その時間窓だけを `--%` と表示します。

Gitリポジトリは作業ディレクトリから親方向へ `.git` を探索します。通常のリポジトリでは `git diff --numstat HEAD --` により、`HEAD` と比較したステージ済み・未ステージの追跡対象変更を集計します。初回コミット前のリポジトリでは `git diff --cached --numstat <empty-tree> --` により、ステージ済み変更だけを空ツリーと比較します。どちらも3秒で打ち切り、未追跡ファイルは差分統計に含めません。

無効なJSONや内部例外はClaude CodeのUIを妨げないよう静かに処理され、プロセスは終了コード0で終了します。

### `--subagent` モード

最初の引数が `--subagent` の場合、入力JSONの `tasks` 配列を処理します。表示可能な各タスクについて、次の形式を1行ずつ出力します。

```json
{"id":"<task id>","content":"<ANSI装飾済みの表示文字列>"}
```

`content` には、存在する場合にモデル、Effort、トークン数またはコンテキスト使用率、説明を含めます。

## スモークテスト

インストール後、Claude Codeを起動する前に直接実行できます。

### Windows PowerShell

通常モード:

```powershell
$binary = "$HOME/.local/bin/claude-statusline/StatusLinePowerline.exe"
'{"model":{"display_name":"Example"},"workspace":{"current_dir":"C:\\work"},"context_window":{"total_input_tokens":12000,"context_window_size":200000},"rate_limits":{"five_hour":{"used_percentage":10},"seven_day":{"used_percentage":20}}}' |
  & $binary
```

サブエージェントモード:

```powershell
$binary = "$HOME/.local/bin/claude-statusline/StatusLinePowerline.exe"
'{"columns":80,"tasks":[{"id":"task-1","model":"claude-opus-5","effort":"high","tokenCount":12000,"contextWindowSize":200000,"description":"status check"}]}' |
  & $binary --subagent
```

### Linux/macOS

通常モード:

```sh
binary="$HOME/.local/bin/claude-statusline/StatusLinePowerline"
printf '%s\n' '{"model":{"display_name":"Example"},"workspace":{"current_dir":"/tmp/project"},"context_window":{"total_input_tokens":12000,"context_window_size":200000}}' |
  "$binary"
```

サブエージェントモード:

```sh
binary="$HOME/.local/bin/claude-statusline/StatusLinePowerline"
printf '%s\n' '{"tasks":[{"id":"task-1","model":"claude-opus-5","effort":"high","tokenCount":12000,"contextWindowSize":200000,"description":"status check"}]}' |
  "$binary" --subagent
```

通常モードはANSI装飾済みの1本のstatuslineを返します。`--subagent` は `{ "id", "content" }` のJSONオブジェクトを1行ずつ返します。色やグリフの見え方はターミナルとフォントに依存します。

## トラブルシューティング

| 現象 | 確認・対処 |
| --- | --- |
| `dotnet build` 後も表示が古い | `build` は設定が参照するインストール先を更新しません。正しいRIDと `-o` で、設定内のパスへ再度 `dotnet publish` します。 |
| statuslineが表示されない | 実行ファイルをスモークテストで直接実行し、絶対パス、設定JSON、.NET 10 Runtimeを確認します。 |
| `Permission denied`（Linux/macOS） | `chmod 755 "$HOME/.local/bin/claude-statusline/StatusLinePowerline"` を実行します。ホームが `noexec` の場合は別のユーザー所有ディレクトリへ配置し、設定の絶対パスも更新します。 |
| `Exec format error` | 実行先のOS・CPUと異なるRIDの成果物です。対象に合うRIDで再publishします。 |
| Windowsでコマンドを実行できない | Windows設定例どおり前方スラッシュのパスを使い、PowerShellの `&` 呼び出し演算子を含めます。 |
| 色や区切りが崩れる | ANSI truecolor対応端末とPowerline/Nerd Fontを確認します。 |
| モデル、Effort、コンテキストが `?` や `--` になる | Claude Codeから渡されたJSONの該当フィールドがないか、形式が異なります。 |
| 利用率が `--%` になる | 入力の `rate_limits` と、任意の `~/.claude.json` キャッシュを確認します。 |
| Gitブランチやワークツリーが出ない | 作業ディレクトリまたは親に読み取れる `.git` があることを確認します。 |
| Git差分だけが出ない | `git` が `PATH` 上にあること、差分があること、3秒以内に完了することを確認します。未追跡ファイルは対象外です。 |
| `--subagent` が何も返さない | `tasks` が配列で、各対象に空でない `id` と表示可能なモデル・Effort・トークン情報があることを確認します。 |

## 検証状況

このドキュメントの作成時に、次を確認しています。

- `dotnet build StatusLinePowerline.csproj -c Release`: 成功、警告0、エラー0
- 一時ディレクトリへのcross-RID publish:
  - `win-x64`: 成功
  - `win-arm64`: 成功
  - `linux-x64`: 成功
  - `linux-arm64`: 成功
  - `osx-x64`: 成功
  - `osx-arm64`: 成功
- `win-x64`成果物の通常モード: 終了コード0、標準エラーなし
- `win-x64`成果物の `--subagent` モード: 終了コード0、標準エラーなし、JSON Linesを確認
- README内のJSON設定例: 構文確認済み

cross-RID publishの成功は、対象OS上でのネイティブ実行確認ではありません。Linux/macOSの実行、ターミナル表示、Claude Code経由の動作は、それぞれの実機で別途確認してください。

## 参考資料

- [Customize your status line](https://code.claude.com/docs/en/statusline)
- [Claude Code settings](https://code.claude.com/docs/en/settings)
- [Environment variables](https://code.claude.com/docs/en/env-vars)
