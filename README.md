# StatusLinePowerline

`StatusLinePowerline` は、Claude Code から標準入力で渡される JSON を読み、ANSI カラー付きの Powerline 形式ステータス行を標準出力へ返す Rust 製のコンソールアプリケーションです。

通常の `statusLine` では、次の情報を左から順に表示します。

1. `ユーザー名@ホスト名`
2. 作業ディレクトリ
3. モデル
4. Effort（段階名と5段ゲージ）
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

### Effort の段数ゲージ

Effort は割合ではなく `low` / `medium` / `high` / `xhigh` / `max` の5段階です。`minimal` という段階は存在せず、`xhigh` の上が `max` です。ultracode は独立した段階ではなく `xhigh` として報告されます。連続値ではないので使用率バーとは描き分け、**5つの台形を斜めの境界でつないだゲージ**として表示します。

各段は「区切り1マス＋地1マス」でできています。区切りの前景に前の段の色、背景に次の段の色を置くことで、独立した四角ではなく連続した台形に見えます。ゲージの前後でセグメント本来の色へ戻すため、外側のPowerline接続には影響しません。

点灯セルは明るい地の面、未点灯セルは暗い地に `░` を置きます。`░` は使用率バーの未使用部分と同じ記号で、**色以外の手がかり**を足すことで点灯・未点灯の別と段数の両方を読み取れるようにしています。

### Effort ゲージの配色

ここでは性質の違う2つの見分けが同時に必要になります。**片方だけを最適化すると、もう片方が読めなくなります。**

| 見分けたいもの | 効く指標 | 理由 |
| --- | --- | --- |
| 点灯 と 未点灯 | **輝度比** | 1マス幅の図と地の判別。暗い色どうしは色差が大きくても分離しない |
| 段 と 隣の段 | **色差** | 数を数えるための境界。輝度差は小さくてよい |

当初は色差だけを見て設計したため、点灯色を暗い側から始めるランプにしてしまい、段1の輝度比が 1.80 しかありませんでした。色差は 39.8 あっても、**一番見たい低い段階ほど点灯が読み取れない**という配分になっていました。点灯色は「確実に明るい」帯の中だけでランプを組むよう改めています。

| 段 | 点灯色 | L* | 未点灯との輝度比 |
| --- | --- | --- | --- |
| 1 | `#8767B8` | 49.9 | 3.64 |
| 2 | `#9F7FCC` | 58.9 | 4.99 |
| 3 | `#B699DF` | 68.1 | 6.73 |
| 4 | `#CDB3EE` | 76.9 | 8.82 |
| 5 | `#E2D0F8` | 86.0 | 11.42 |
| 未点灯 | `#231D29` | 11.9 | — |

段どうしの色差は 9.7 〜 14.3 で、1本のランプとして読ませつつ境界が数えられる程度に保っています。5色を別々の色相にはしていません。高い Effort ほど明るい段まで届くため、ゲージ全体の明度が上がって見えます。

未点灯どうしは背景が**文字どおり同一**なので、実線の楔は原理的に描けません。ここだけは細い区切り `` を `#786491` で描きます。セグメント間の境界に細い区切りを使わないのとは事情が異なります。

`--` や未知の値のときは、段を決められないためゲージを出さず段階名だけを表示します。Effort に対応しないモデルでは入力に `effort` 自体が現れないため、この場合も `--` になります。

### 使用率バーの配色

コンテキストと利用率のセグメントは、**セグメント背景色**（しきい値ごとの帯）と**バーの塗り色**（緑→黄→赤の連続グラデーション）の2つで使用率を表しています。この2つを同じ面に重ねると、使用率が上がるほど両者の色相が近づき、最悪の場合は赤地に赤となって判読できなくなります。

そのため、バーは**専用の暗いトラック**（`#1A1E26`）の上に描きます。バーの区間だけ背景を差し替え、直後にセグメント本来の背景色へ戻すため、Powerlineの区切りには影響しません。

| | バー塗り色 vs その背景のコントラスト比 |
| --- | --- |
| トラックなし（重ねた場合） | 1.00 〜 2.00 |
| トラックあり（現在） | 4.23 〜 10.80 |

未使用部分は `░` を `#4E5666` で描きます。トラックとのコントラスト比は 2.26 で、存在は分かるが塗り部分と競合しない濃さです。

### セグメントの配色と境界

背景色はいずれも、文字色に対してコントラスト比 5.4 以上を確保しています。

中間輝度の背景色は、文字を白にしても黒にしても比が上がらないため避けています。たとえば以前使っていたティール `#168777` は、純黒でも 4.77、純白でも 4.41 が上限で、どちらを選んでも WCAG AA（4.5）に届きませんでした。背景色は「白文字が乗る暗い色」か「黒文字が乗る明るい色」のどちらかに寄せています。

| セグメント | 明暗 | 背景 | 文字 | 比 |
| --- | --- | --- | --- | --- |
| `ユーザー名@ホスト名` | 暗 | `#2D6A4F` | 白 | 6.39 |
| 作業ディレクトリ | 暗 | `#6F4EB0` | 白 | 6.19 |
| モデル | 暗 | `#B93131` | 白 | 5.92 |
| Effort | 暗 | `#6C3A84` | 白 | 8.15 |
| コンテキスト 〜70% | 明 | `#CD9A0B` | 黒 | 6.11 |
| コンテキスト 70〜85% | 明 | `#F5C418` | 黒 | 9.51 |
| コンテキスト 85〜95% | 明 | `#F58A2B` | 黒 | 6.36 |
| コンテキスト 95%〜 | 明 | `#FF6B5B` | 黒 | 5.58 |
| 5時間 〜50% | 暗 | `#10554A` | 白 | 8.68 |
| 5時間 50〜80% | 暗 | `#7A5410` | 白 | 6.77 |
| 5時間 80%〜 | 暗 | `#9E281E` | 白 | 7.56 |
| 7日 〜50% | 明 | `#48C7B0` | 黒 | 7.49 |
| 7日 50〜80% | 明 | `#CD9A0B` | 黒 | 6.11 |
| 7日 80%〜 | 明 | `#FF6B5B` | 黒 | 5.58 |
| ブランチ | 暗 | `#1D5A82` | 白 | 7.40 |
| ワークツリー | 暗 | `#5C48A5` | 白 | 7.18 |
| 差分行数 | 明 | `#27AE60` | 黒 | 5.43 |

### セグメントの境界

Powerlineの区切り `` は、**前のセグメントの背景色**で描かれます。つまり区切りは「前のセグメントが尖って次のセグメントへ食い込む」形であり、隣接する背景色が同じだと区切りそのものが見えなくなります。

コンテキスト・5時間・7日は必ずこの順で隣り合い、しきい値も連動するため、単一のパレットを共有すると同じ色が並びます。これを避けるため、コンテキストと7日には明るい系、5時間には暗い系を割り当て、**この3つが常に「明 → 暗 → 明」と交互**になるようにしています。5時間と7日は同じ色相で明暗だけを変えており、同種の指標であることは保ったまま境界が出ます。

境界が見えるかどうかの判定に **WCAG のコントラスト比を使ってはいけません**。あれは輝度だけの指標で色相差を見ないため、たとえば緑 `#2D6A4F` と紫 `#6F4EB0` はコントラスト比 1.03（＝ほぼ同じ）と判定されますが、人間には全く別の色に見えます。判定には CIE Lab の色差を使います。

| 隣接ペア | コントラスト比 | 色差 |
| --- | --- | --- |
| `ユーザー名@ホスト名` → 作業ディレクトリ | 1.03 | **84.7** |
| ブランチ → ワークツリー | 1.03 | **41.5** |
| 隣接ペア全35通りの最小値 | 1.03 | **29.1** |

色差は 2.3 前後でようやく違いが分かる程度、10 を超えれば誰が見ても別の色です。現在の配色では全ての隣接ペアが 29.1 以上あるため、区切りは常に実線のままで問題ありません。

この条件は `cargo test` で機械的に検証しています。

| テスト | 内容 |
| --- | --- |
| `adjacent_backgrounds_are_perceptually_distinct` | 隣接しうる背景色の全組み合わせで色差 20 以上 |
| `segment_text_meets_wcag_aa` | 全セグメントの文字コントラスト比 4.5 以上 |
| `usage_bar_is_legible_on_its_track` | 0〜100%の全域でバーとトラックのコントラスト比 4.0 以上 |

コンテキストと7日の危険帯はどちらも `#FF6B5B` ですが、あいだに5時間が入るため隣接しません。

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

このセクションは、現在の実装に至るまでの判断と、その理由を残すためのものです。

### C# から Rust への移植

当初は .NET 10 / C# で書かれていました（`Program.cs`、`StatusLinePowerline.csproj`）。移植の動機は2つです。

- 実行先に .NET Runtime を要求する（`--self-contained false` の場合）
- statusline は更新のたびに起動するため、起動コストが表示の体感に直結する

Rust ではランタイム不要の単体バイナリになり、起動も数ミリ秒で済みます。外部から見た動作は変えていません。

移植にあたり、C# 版と Rust 版を並行維持する案も検討しましたが、**同一仕様1000行超の2実装は必ずズレる**ため一本化しました。C# 版はgit履歴に残っています。

一点だけ後退した部分があります。.NET は Linux 上から `-r osx-arm64` で macOS 向けバイナリを publish できましたが、Rust では Apple SDK が必要なため実質できません。macOS 版が必要になった場合は、macOSランナー上でビルドしてください。

### 表示項目の整理

- 項目名（`Model:`、`Effort:`、`Ctx:`、`Cwd:`）を削除。セグメントの色と位置で区別できるため、幅を使うだけでした
- `ユーザー名@ホスト名` を追加し、先頭2つをシェルの `PS1`（`\u@\h:\w`）と同じ並びに
- ホスト名の取得にクレートを追加する案は見送りました。`gethostname` は rustix を含む5つの推移的依存を持ち込みますが、必要なのはシステムコール1回分です

### Effort の段数ゲージ

Effort は割合ではなく `low` / `medium` / `high` / `xhigh` / `max` の5段階です。`minimal` という段階は存在せず、`xhigh` の上が `max` です。ultracode は独立した段階ではなく `xhigh` として報告されます。連続値ではないので使用率バーとは描き分け、**5つの台形を斜めの境界でつないだゲージ**として表示します。

```text
minimal  1/5      low  2/5      medium  3/5      high  4/5      xhigh  5/5
```

各段は「区切り1マス＋地1マス」でできています。区切りの前景に前の段の色、背景に次の段の色を置くことで、独立した四角ではなく連続した台形に見えます。ゲージの前後でセグメント本来の色へ戻すため、外側のPowerline接続には影響しません。

塗り色は Effort セグメントと同じ紫系のランプで、**位置が上がるほど明るく**なります。段ごとに別々の色相にはしていません。高い Effort ほど明るい段まで届くため、ゲージ全体の明度が上がって見えます。

| 段 | 塗り色 | L* |
| --- | --- | --- |
| 1 | `#6A3E9E` | 35.9 |
| 2 | `#8055B8` | 45.0 |
| 3 | `#966DD0` | 54.0 |
| 4 | `#AC86E2` | 62.8 |
| 5 | `#C4A2F2` | 72.2 |
| 未点灯 | `#35284A` | 19.1 |

境界の色差は次のとおりです。塗り同士は1本のランプとして読ませたいので控えめに、水位（塗り→未点灯）は一目で分かるように離しています。

| 境界 | 色差 |
| --- | --- |
| 塗り → 隣の塗り | 9.1 〜 12.3 |
| 塗り → 未点灯（水位） | 39.8 〜 57.2 |
| セグメント背景 → 塗り1 | 13.1 |

未点灯どうしは背景が**文字どおり同一**なので、実線の楔は原理的に描けません。ここだけは細い区切り `` を `#786491` で描き、5段あることが分かるようにしています。セグメント間の境界に細い区切りを使わないのとは事情が異なります。

`--` や未知の値のときは、段を決められないためゲージを出さず段階名だけを表示します。Effort に対応しないモデルでは入力に `effort` 自体が現れないため、この場合も `--` になります。

### 使用率バーの配色

セグメント背景色（しきい値の帯）とバーの塗り色（緑→赤のグラデーション）が**どちらも使用率を符号化していた**ため、使用率が上がるほど両者の色相が接近し、69%でコントラスト比 1.00（輝度が完全一致）、100%で 1.03 になっていました。

4案を比較しました。

| 案 | 内容 | 結果 |
| --- | --- | --- |
| A | グラデーションを廃止し、背景の明暗に応じた単色にする | 読めるが、緑→赤の情報が失われる |
| B | グラデーションを維持し、輝度だけ背景から強制的に離す | 補正方向が背景ごとに反転し、隣接セグメントで暗いバーと淡いバーが混在する |
| C | セグメント背景をニュートラルにし、バーに色を集約する | 情報設計としては最良だが、Powerlineの色数が大幅に減る |
| **D** | **バー専用の暗いトラックを敷く** | **採用。** グラデーションは原色のまま、全セグメント共通で 4.23 以上 |

### 文字コントラスト

中間輝度の背景色は、文字を白にしても黒にしても比が上がりません。たとえばティール `#168777` は純黒でも 4.77、純白でも 4.41 が上限で、どちらを選んでも WCAG AA（4.5）に届きませんでした。背景色は「白文字が乗る暗い色」か「黒文字が乗る明るい色」のどちらかに寄せる方針に変更し、最小値を 2.87 から 5.43 へ引き上げました。

### セグメント境界

コンテキスト・5時間・7日は必ずこの順で隣り合い、しきい値も連動します。単一のパレットを共有していたため、**5組が完全に同色**になり、区切りが消えていました。5時間を暗い系に振り分けて「明 → 暗 → 明」の交互配色にすることで解決しています。

ここで一度、**判定基準そのものを間違えました**。近すぎる隣接には細い区切り `` を描くフォールバックを入れたのですが、その判定に WCAG のコントラスト比を使っていました。あれは輝度だけの指標で色相を見ないため、緑 `#2D6A4F` と紫 `#6F4EB0` を「ほぼ同じ」（比 1.03）と判定し、本来描かれるべき緑の楔を消してしまっていました。両者の CIE Lab 色差は 84.7 で、人間には全く別の色です。

CIE Lab で測り直したところ隣接ペア全35通りの最小色差が 29.1 あったため、フォールバックごと削除し、区切りは常に実線に戻しました。

### テストによる固定

上記の配色条件は目視では守れないため、`cargo test` で機械的に検証しています。旧パレットへ戻すと `色差 0.0` を検出して失敗することも確認済みです。

## 検証状況

`x86_64-unknown-linux-gnu`（Ubuntu 22.04、glibc 2.35、Rust 1.97.1）で次を確認しています。

- `cargo build --release`: 成功、警告0、エラー0
- `cargo clippy --release --all-targets`: 指摘0
- 成果物: 653KB、動的依存は `libc` と `libgcc` のみ
- 起動時間: 約14ms/回（`git diff` 呼び出しを含む実リポジトリ上での50回平均）
- 通常モード: 終了コード0、標準エラーなし
- `ユーザー名@ホスト名`: `/proc/sys/kernel/hostname` から短縮ホスト名を取得できることを確認
- 使用率バー: トラック背景への切り替えと復帰のANSIシーケンスを確認。全使用率でコントラスト比 4.23 以上
- セグメントの文字コントラスト: 全17パターンを計測し、最小 5.43（変更前の最小は 2.87）
- セグメント境界: 隣接しうる背景色35通りの最小色差 29.1（変更前は 0.0＝完全同色が5組）
- Effort ゲージ: 5段階すべてで点灯セル数・未点灯セル数・区切り6個が一致することを確認。点灯と未点灯の輝度比 3.64 以上、段どうしの色差 9.7 以上
- `cargo test --release`: 9件成功。旧パレットへ戻すと色差 0.0 を検出して失敗することも確認
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
