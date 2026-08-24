//! Powerline status line for Claude Code.
//!
//! Rust port of the C# `Program.cs` in the repository root. The observable
//! contract is unchanged: stdin receives one JSON object, stdout receives the
//! rendered line (or JSON Lines with `--subagent`), and the process always
//! exits 0 so a failure here can never disturb Claude Code's UI.

use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc};
use serde_json::Value;

const ESCAPE: &str = "\x1b[";
// セグメント間の区切りは常に実線。前のセグメントの背景色で描かれるので、
// 隣り合う背景が知覚的に十分離れている必要がある。この条件は tests で
// 機械的に検証している。
const POWERLINE_RIGHT: char = '\u{e0b0}';
// 同じ背景どうしを区切る細い楔。黒い空セルのあいだだけで使う。
const POWERLINE_RIGHT_THIN: char = '\u{e0b1}';
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

#[derive(Clone, Copy, PartialEq, Eq)]
struct Color {
    red: i32,
    green: i32,
    blue: i32,
}

const fn rgb(red: i32, green: i32, blue: i32) -> Color {
    Color { red, green, blue }
}

const MODEL_BACKGROUND: Color = rgb(185, 49, 49);
const EFFORT_BACKGROUND: Color = rgb(108, 58, 132);
const CONTEXT_LOW_BACKGROUND: Color = rgb(205, 154, 11);
const CONTEXT_MEDIUM_BACKGROUND: Color = rgb(245, 196, 24);
const CONTEXT_HIGH_BACKGROUND: Color = rgb(245, 138, 43);
const CONTEXT_CRITICAL_BACKGROUND: Color = rgb(255, 107, 91);
const DIRECTORY_BACKGROUND: Color = rgb(111, 78, 176);
// 利用率は 5h と 7d が必ず隣り合う。同じしきい値なら同じ色になってしまうため、
// 5h には暗い系（白文字）、7d には明るい系（黒文字）を割り当てて明暗を交互にする。
// コンテキストも明るい系なので、Ctx → 5h → 7d は常に 明 → 暗 → 明 と並ぶ。
const RATE_DARK_LOW_BACKGROUND: Color = rgb(16, 85, 74);
const RATE_DARK_MEDIUM_BACKGROUND: Color = rgb(122, 84, 16);
const RATE_DARK_HIGH_BACKGROUND: Color = rgb(158, 40, 30);
const RATE_LIGHT_LOW_BACKGROUND: Color = rgb(72, 199, 176);
const RATE_LIGHT_MEDIUM_BACKGROUND: Color = rgb(205, 154, 11);
const RATE_LIGHT_HIGH_BACKGROUND: Color = rgb(255, 107, 91);
const GIT_BACKGROUND: Color = rgb(29, 90, 130);
const WORKTREE_BACKGROUND: Color = rgb(92, 72, 165);
const DIFF_BACKGROUND: Color = rgb(39, 174, 96);
const IDENTITY_BACKGROUND: Color = rgb(45, 106, 79);
const COST_BACKGROUND: Color = rgb(176, 187, 200);
const WHITE: Color = rgb(255, 255, 255);
const DARK_TEXT: Color = rgb(30, 36, 45);

// 使用率バーは専用の暗いトラックの上に描く。セグメント背景も使用率で変わるため、
// 同じ面にグラデーションを載せると高使用率で赤地に赤となり視認できなくなる。
// トラックを挟むことで、どのセグメントでもコントラスト比 4.2 以上を保証する。
const BAR_TRACK_BACKGROUND: Color = rgb(26, 30, 38);
const BAR_EMPTY_FOREGROUND: Color = rgb(78, 86, 102);

// Effort は連続値ではなく5段階なので、割合を示すバーではなく5セルの段数
// ゲージで描く。現在の段階までを紫で描き、上の段も黒い空セルとして残す。
// 塗りは1本の紫系ランプで、位置が上がるほど明るい。Effort が高いほど
// 明るい段まで届き、ゲージ全体が強く見える。
const EFFORT_STEPS: usize = 5;
// ゲージは現在の段階まで1段ずつ点いていき、そこで止まる。段は2つの状態を取る。
//
//   点灯   すでに点いた段。彩度の高い紫
//   待機   現在の段階の範囲内だが、まだ点いていない段。点灯と同じ明度で低彩度
//
// 待機色を「同じ明度・低彩度」にしているのは、CIE Lab では輝度が L* だけで
// 決まるためで、彩度を落としても点灯の並びの明度は変わらない。おかげで
// アニメーションのどのコマでも現在の段階が読み取れる。明度の並び（段が
// 上がるほど明るい）も崩れない。
const EFFORT_LIT_BACKGROUNDS: [Color; EFFORT_STEPS] = [
    rgb(135, 103, 184),
    rgb(159, 127, 204),
    rgb(182, 153, 223),
    rgb(205, 179, 238),
    rgb(226, 208, 248),
];
const EFFORT_PENDING_BACKGROUNDS: [Color; EFFORT_STEPS] = [
    rgb(123, 116, 131),
    rgb(146, 140, 153),
    rgb(169, 164, 176),
    rgb(193, 188, 199),
    rgb(217, 214, 221),
];
// 満ちきったあと次の周回まで留まるコマ数。再描画は毎秒1回が上限なので、
// これがそのまま「止まって見える秒数」になる。
const EFFORT_HOLD_FRAMES: usize = 4;
// 現在の段階より上も、文字を置かない黒いセルとして残す。
const EFFORT_EMPTY_BACKGROUND: Color = rgb(35, 29, 41);
// 同色の空セルの境界だけは実線の楔にできないため、細い区切りを描く。
const EFFORT_EMPTY_DIVIDER: Color = rgb(120, 100, 145);

struct Segment {
    background: Color,
    foreground: Color,
    text: String,
}

struct Context {
    current: String,
    maximum: String,
    percentage: String,
    percentage_value: Option<f64>,
}

#[derive(Clone, Copy, Default)]
struct RateLimit {
    percentage: Option<f64>,
    resets_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Default)]
struct RateLimits {
    five_hour: RateLimit,
    seven_day: RateLimit,
}

struct Repository {
    branch: Option<String>,
    worktree: Option<String>,
    is_unborn: bool,
}

/// 利用率セグメントの明暗。5h と 7d は必ず隣り合うため、同じしきい値でも
/// 背景が一致しないよう、片方を暗い系、もう片方を明るい系に固定する。
#[derive(Clone, Copy)]
enum RateScale {
    Dark,
    Light,
}

fn main() {
    // A status line must never interfere with Claude Code's UI, so swallow
    // panics the way the C# implementation swallows exceptions.
    std::panic::set_hook(Box::new(|_| {}));
    let _ = std::panic::catch_unwind(run);
    std::process::exit(0);
}

fn run() {
    let mut raw = Vec::new();
    if std::io::stdin().read_to_end(&mut raw).is_err() {
        return;
    }

    let input = String::from_utf8_lossy(&raw);
    let root = parse_object(&input);
    let subagent = std::env::args().nth(1).as_deref() == Some("--subagent");
    if subagent {
        write_subagent_status(&root);
    } else {
        print!("{}", build_status_line(&root));
    }
}

fn parse_object(input: &str) -> Value {
    match serde_json::from_str::<Value>(input) {
        Ok(value) if value.is_object() => value,
        _ => Value::Object(serde_json::Map::new()),
    }
}

// ---------------------------------------------------------------- status line

fn build_status_line(root: &Value) -> String {
    let directory = get_working_directory(root);
    let context = get_context(root);
    let rates = get_rate_limits(root);
    let repository = find_repository(&directory, get_workspace_worktree(root));
    let main_effort = get_main_effort(root);

    // コンテキストは全帯を明るい系で揃えているため、文字色は常に暗色でよい。
    let context_foreground = DARK_TEXT;
    let context_background = get_context_background(context.percentage_value);
    // 並び順はシェルの PS1 と同じく `\u@\h` → `\w` から始め、そのあとに
    // セッション情報（モデル、Effort、コンテキスト）、利用率、Git情報と続く。
    let mut segments = vec![
        Segment {
            background: IDENTITY_BACKGROUND,
            foreground: WHITE,
            text: format!(" {} ", get_identity()),
        },
        Segment {
            background: DIRECTORY_BACKGROUND,
            foreground: WHITE,
            text: format!(" {} ", format_directory(&directory)),
        },
        Segment {
            background: MODEL_BACKGROUND,
            foreground: WHITE,
            text: format!(" {} ", get_model_name(root)),
        },
        Segment {
            background: EFFORT_BACKGROUND,
            foreground: WHITE,
            text: create_effort_text(&main_effort),
        },
        Segment {
            background: context_background,
            foreground: context_foreground,
            text: create_context_text(&context, context_foreground, context_background),
        },
        create_rate_segment("5h", rates.five_hour, RateScale::Dark),
        create_rate_segment("7d", rates.seven_day, RateScale::Light),
    ];

    // Claude Code が渡すのはこのセッションの推定コストだけで、アカウントの
    // 課金上限や当月の使用量は入力に含まれない。上限がないのでバーは置かず、
    // 金額だけを出す。`/clear` で 0 に戻る。
    if let Some(cost) = get_session_cost(root) {
        segments.push(Segment {
            background: COST_BACKGROUND,
            foreground: DARK_TEXT,
            text: format!(" ${} ", format_fixed(cost, 2)),
        });
    }

    if let Some(repository) = repository {
        if let Some(branch) = repository.branch.as_deref() {
            segments.push(Segment {
                background: GIT_BACKGROUND,
                foreground: WHITE,
                text: format!(" \u{2387} {} ", clean_text(branch)),
                });

            if let Some(worktree) = repository.worktree.as_deref() {
                if !is_blank(worktree) {
                    segments.push(Segment {
                        background: WORKTREE_BACKGROUND,
                        foreground: WHITE,
                        text: format!(" WT: {} ", clean_text(worktree)),
                                });
                }
            }

            if let Some((added, deleted)) = get_diff_stat(&repository, &directory) {
                if added > 0 || deleted > 0 {
                    // 明るい緑地なので、白文字ではコントラスト比 2.87 しか出ない。
                    segments.push(Segment {
                        background: DIFF_BACKGROUND,
                        foreground: DARK_TEXT,
                        text: format!(" (+{},-{}) ", added, deleted),
                                });
                }
            }
        }
    }

    render_powerline(&segments)
}

fn get_model_name(root: &Value) -> String {
    let Some(model) = get_object(root, "model") else {
        return "?".to_string();
    };

    let name = get_string(model, "display_name")
        .filter(|value| !is_blank(value))
        .or_else(|| get_string(model, "id"));

    match name {
        Some(name) if !is_blank(name) => clean_text(name),
        _ => "?".to_string(),
    }
}

fn get_main_effort(root: &Value) -> String {
    let effort = get_object(root, "effort")
        .and_then(|effort| get_string(effort, "level"))
        .or_else(|| get_string(root, "effort"))
        .or_else(|| get_string(root, "effortLevel"))
        .or_else(|| get_string(root, "reasoningEffort"));

    let Some(effort) = effort.filter(|value| !is_blank(value)) else {
        return "--".to_string();
    };

    let cleaned = clean_text(effort).trim().to_string();
    if cleaned.is_empty() {
        "--".to_string()
    } else {
        capitalize(&cleaned)
    }
}

/// Effort の表示。既知の5段階なら見出しのあとに、常に5セルのゲージを添える。
/// 未知の値や `--` のときは、段が決められないのでゲージを出さない。
fn create_effort_text(label: &str) -> String {
    match get_effort_step(label) {
        Some(step) => format!(" {} {} ", label, build_effort_gauge(step, current_phase(), Some(EFFORT_BACKGROUND))),
        None => format!(" {} ", label),
    }
}

/// アニメーションのコマ番号。周期は段階ごとに変わるので経過秒をそのまま渡す。
/// 時刻が取れない場合は先頭のコマに落ちるだけで、表示は壊れない。
fn current_phase() -> usize {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as usize)
        .unwrap_or(0)
}

/// Claude Code の Effort は `low` / `medium` / `high` / `xhigh` / `max` の5段階。
/// `minimal` という段階は存在せず、`xhigh` の上が `max` になる。ultracode は
/// 独立した段階ではなく `xhigh` として報告される。対応しないモデルでは
/// `effort` 自体が入力に現れない。
fn get_effort_step(label: &str) -> Option<usize> {
    match label.trim().to_ascii_lowercase().as_str() {
        "low" => Some(1),
        "medium" => Some(2),
        "high" => Some(3),
        "xhigh" => Some(4),
        "max" => Some(5),
        _ => None,
    }
}

/// 最大5段を斜めの境界でつないだゲージ。各段は「区切り＋1マスの地」でできる。
/// 現在の段階までの地は紫、上の段は文字を置かない黒い地にする。同じ黒地が
/// 連続する箇所だけは細い楔で区切るので、5段すべてが読め、各セルの地も途切れない。
/// 最後にセグメント本来の色へ戻すので、外側の Powerline 接続には影響しない。
/// `surround` はゲージの外側の背景色で、Powerline セグメントの中に描くときは
/// その背景色を渡す。サブエージェント行のように端末の地の上へ直接描くときは
/// `None` を渡す。外側の色が分からないとゲージ先頭の楔は描けないので、その
/// 場合は最初の段から始める。
fn build_effort_gauge(step: usize, phase: usize, surround: Option<Color>) -> String {
    // 1コマ目は何も点いていない状態から始め、1段ずつ点けていき、現在の段階に
    // 達したらしばらくそのまま留まる。段階より上のセルも黒い地として残す。
    let cycle = step + 1 + EFFORT_HOLD_FRAMES;
    let lit = (phase % cycle).min(step);

    let mut gauge = String::new();
    let mut previous = surround;

    for index in 0..EFFORT_STEPS {
        let current = if index < step {
            if index < lit {
                EFFORT_LIT_BACKGROUNDS[index]
            } else {
                EFFORT_PENDING_BACKGROUNDS[index]
            }
        } else {
            EFFORT_EMPTY_BACKGROUND
        };

        match previous {
            Some(previous) if previous == current => {
                // 両側が同じ黒地では実線の楔が消える。細い楔も黒地の上で描き、
                // どちらの空セルにも別の背景色を混ぜない。
                gauge.push_str(&foreground(EFFORT_EMPTY_DIVIDER));
                gauge.push_str(&background(current));
                gauge.push(POWERLINE_RIGHT_THIN);
            }
            Some(previous) => {
                gauge.push_str(&foreground(previous));
                gauge.push_str(&background(current));
                gauge.push(POWERLINE_RIGHT);
            }
            None => gauge.push_str(&background(current)),
        }

        gauge.push(' ');
        previous = Some(current);
    }

    let last = previous.unwrap_or(EFFORT_EMPTY_BACKGROUND);
    gauge.push_str(&foreground(last));
    match surround {
        Some(color) => {
            gauge.push_str(&background(color));
            gauge.push(POWERLINE_RIGHT);
            gauge.push_str(&foreground(WHITE));
        }
        None => {
            // 端末の地へ戻してから閉じ、装飾を残さないよう完全にリセットする。
            gauge.push_str(ESCAPE);
            gauge.push_str("49m");
            gauge.push(POWERLINE_RIGHT);
            gauge.push_str(ESCAPE);
            gauge.push_str("0m");
        }
    }
    gauge
}

fn create_context_text(context: &Context, foreground: Color, background: Color) -> String {
    match context.percentage_value {
        None => format!(" {}/{} --% ", context.current, context.maximum),
        Some(value) => format!(
            " {}/{} {} {}% ",
            context.current,
            context.maximum,
            build_usage_bar(value, foreground, background),
            context.percentage
        ),
    }
}

fn build_usage_bar(percentage: f64, segment_foreground: Color, segment_background: Color) -> String {
    const WIDTH: i32 = 10;
    const BLOCKS: [char; 9] = [
        ' ', '\u{258f}', '\u{258e}', '\u{258d}', '\u{258c}', '\u{258b}', '\u{258a}', '\u{2589}',
        '\u{2588}',
    ];

    let clamped = percentage.clamp(0.0, 100.0);
    let filled = clamped * f64::from(WIDTH) / 100.0;
    let full = WIDTH.min(filled.floor() as i32);
    let fraction = if full == WIDTH {
        0
    } else {
        ((filled - f64::from(full)) * 8.0).floor() as i32
    };
    let empty = WIDTH - full - i32::from(fraction > 0);

    let mut filled_glyphs = String::new();
    for _ in 0..full.max(0) {
        filled_glyphs.push(BLOCKS[8]);
    }
    if fraction > 0 {
        filled_glyphs.push(BLOCKS[fraction as usize]);
    }

    let mut empty_glyphs = String::new();
    for _ in 0..empty.max(0) {
        empty_glyphs.push('\u{2591}');
    }

    // The bar sits on its own dark track so the gradient never has to compete
    // with a segment background that encodes the same usage level. Both the
    // track background and the bar foreground are restored to the segment's own
    // colours afterwards, leaving the Powerline connection untouched.
    format!(
        "{}{}{}{}{}{}{}",
        background(BAR_TRACK_BACKGROUND),
        foreground(get_usage_gradient(clamped)),
        filled_glyphs,
        foreground(BAR_EMPTY_FOREGROUND),
        empty_glyphs,
        background(segment_background),
        foreground(segment_foreground)
    )
}

fn get_usage_gradient(percentage: f64) -> Color {
    let clamped = percentage.clamp(0.0, 100.0);
    if clamped < 50.0 {
        rgb((clamped * 5.1) as i32, 200, 80)
    } else {
        rgb(255, ((200.0 - (clamped - 50.0) * 4.0) as i32).max(0), 60)
    }
}

fn get_context(root: &Value) -> Context {
    let Some(context_window) = get_object(root, "context_window") else {
        return Context {
            current: "--".to_string(),
            maximum: "--".to_string(),
            percentage: "--".to_string(),
            percentage_value: None,
        };
    };

    // Claude Code's combined total is the current input context, while current_usage
    // is its component breakdown. Output tokens are deliberately never included.
    let current_value = get_number(context_window, "total_input_tokens")
        .or_else(|| get_current_usage_total(context_window));
    let maximum_value = get_number(context_window, "context_window_size");
    let raw_percentage = get_number(context_window, "used_percentage").or_else(|| {
        match (current_value, maximum_value) {
            (Some(current), Some(maximum)) if maximum > 0.0 => Some(current / maximum * 100.0),
            _ => None,
        }
    });

    // The same away-from-zero display value drives text, colour, and its bar.
    let percentage_value = raw_percentage.map(normalize_percentage);
    Context {
        current: current_value.map_or_else(|| "--".to_string(), format_compact_number),
        maximum: maximum_value.map_or_else(|| "--".to_string(), format_compact_number),
        percentage: percentage_value.map_or_else(|| "--".to_string(), format_percentage),
        percentage_value,
    }
}

fn get_current_usage_total(context_window: &Value) -> Option<f64> {
    let usage = get_object(context_window, "current_usage")?;

    let mut found = false;
    let mut total = 0.0;
    total += first_number(usage, &["input_tokens", "input"], &mut found);
    total += first_number(usage, &["cache_creation_input_tokens", "cache_creation"], &mut found);
    total += first_number(usage, &["cache_read_input_tokens", "cache_read"], &mut found);
    if found {
        Some(total)
    } else {
        None
    }
}

fn first_number(element: &Value, property_names: &[&str], found: &mut bool) -> f64 {
    for property_name in property_names {
        if let Some(value) = get_number(element, property_name) {
            *found = true;
            return value;
        }
    }

    0.0
}

fn get_context_background(percentage: Option<f64>) -> Color {
    match percentage {
        Some(value) if value >= 95.0 => CONTEXT_CRITICAL_BACKGROUND,
        Some(value) if value >= 85.0 => CONTEXT_HIGH_BACKGROUND,
        Some(value) if value >= 70.0 => CONTEXT_MEDIUM_BACKGROUND,
        _ => CONTEXT_LOW_BACKGROUND,
    }
}

// ------------------------------------------------------------ number formats

fn round_half_away_from_zero(value: f64) -> f64 {
    // f64::round already breaks ties away from zero, matching MidpointRounding.
    value.round()
}

fn format_fixed(value: f64, decimals: u32) -> String {
    let factor = 10f64.powi(decimals as i32);
    let rounded = round_half_away_from_zero(value * factor) / factor;
    format!("{:.*}", decimals as usize, rounded)
}

fn format_integer(value: f64) -> String {
    if !value.is_finite() {
        return "?".to_string();
    }

    let rounded = round_half_away_from_zero(value);
    if rounded.abs() < 9.0e15 {
        format!("{}", rounded as i64)
    } else {
        format!("{:.0}", rounded)
    }
}

fn format_compact_number(value: f64) -> String {
    if !value.is_finite() {
        return "?".to_string();
    }

    let absolute = value.abs();
    if absolute >= 1_000_000.0 {
        return format_fixed(value / 1_000_000.0, 1) + "M";
    }

    if absolute >= 1_000.0 {
        // "0.#": at most one decimal, dropped when it is zero.
        let text = format_fixed(value / 1_000.0, 1);
        let trimmed = text.strip_suffix(".0").unwrap_or(&text);
        return trimmed.to_string() + "k";
    }

    format_integer(value)
}

fn normalize_percentage(value: f64) -> f64 {
    round_half_away_from_zero(value)
}

fn format_percentage(value: f64) -> String {
    format_integer(normalize_percentage(value))
}

fn format_rounded_k(value: f64) -> String {
    format_integer(value / 1_000.0) + "k"
}

fn format_rounded_context(value: f64) -> String {
    if value >= 1_000_000.0 {
        format_integer(value / 1_000_000.0) + "M"
    } else {
        format_integer(value / 1_000.0) + "k"
    }
}

// ------------------------------------------------------------------ directory

fn get_working_directory(root: &Value) -> PathBuf {
    let directory = get_object(root, "workspace")
        .and_then(|workspace| get_string(workspace, "current_dir"))
        .filter(|value| !is_blank(value))
        .or_else(|| get_string(root, "cwd"))
        .filter(|value| !is_blank(value));

    match directory {
        Some(directory) => full_path(directory).unwrap_or_else(current_directory),
        None => current_directory(),
    }
}

fn current_directory() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
}

/// Lexical equivalent of `Path.GetFullPath`: absolute and `.`/`..` resolved,
/// without following symlinks the way `fs::canonicalize` would.
fn full_path(path: &str) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }

    let mut resolved = current_directory();
    for component in Path::new(path).components() {
        match component {
            Component::Prefix(prefix) => resolved = PathBuf::from(prefix.as_os_str()),
            Component::RootDir => resolved.push("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(part) => resolved.push(part),
        }
    }

    Some(resolved)
}

fn format_directory(directory: &Path) -> String {
    let display = clean_text(&directory.to_string_lossy());
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return display;
    };

    let home = trim_trailing_separators(&home.to_string_lossy());
    let full = trim_trailing_separators(&directory.to_string_lossy());
    if home.is_empty() {
        return display;
    }

    if full.eq_ignore_ascii_case(&home) {
        return "~".to_string();
    }

    let home_prefix = home.clone() + "/";
    if full.len() >= home_prefix.len() && full[..home_prefix.len()].eq_ignore_ascii_case(&home_prefix)
    {
        return "~".to_string() + &full[home.len()..];
    }

    display
}

fn trim_trailing_separators(value: &str) -> String {
    let trimmed = value.trim_end_matches('/');
    if trimmed.is_empty() && value.starts_with('/') {
        // Keep `/` distinguishable from an empty home directory.
        return String::new();
    }

    trimmed.to_string()
}

/// シェルの `PS1` に含まれる `\u@\h` に相当する識別子。
/// どちらか一方しか判定できない場合は、判定できたほうだけを表示する。
fn get_identity() -> String {
    let user = ["USER", "LOGNAME", "USERNAME"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .map(|value| clean_text(value.trim()))
        .filter(|value| !value.is_empty());

    match (user, read_short_hostname()) {
        (Some(user), Some(host)) => format!("{}@{}", user, host),
        (Some(user), None) => user,
        (None, Some(host)) => host,
        (None, None) => "?".to_string(),
    }
}

/// `\h` と同じく、FQDN ではなく最初のドットより前だけを返す。
fn read_short_hostname() -> Option<String> {
    let raw = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .or_else(|_| std::fs::read_to_string("/etc/hostname"))
        .ok()
        .or_else(|| {
            ["HOSTNAME", "HOST", "COMPUTERNAME"]
                .iter()
                .find_map(|name| std::env::var(name).ok())
        })?;

    let short = clean_text(raw.trim().split('.').next()?);
    let short = short.trim().to_string();
    if short.is_empty() {
        None
    } else {
        Some(short)
    }
}

/// サブエージェント行の入力には Effort が入っていない。タスクが自前の Effort を
/// 持つのは明示指定されたときだけで、セッションの値を継承している場合は
/// `task.effort` が省略される。さらに、メインの statusline には届く
/// トップレベルの `effort` も、statusline の再描画は tool-use コンテキストの
/// イベントではないため送られてこない。
///
/// 入力に含まれる `transcript_path` から実際に使われている Effort を読み戻す。
/// 会話ログの末尾から、sidechain（サブエージェント自身の発話）でない
/// assistant 行を1件見つけるだけなので、全体を読む必要はない。
/// 形式は公開仕様ではないので、読めなければゲージを出さないだけに留める。
fn get_transcript_effort(root: &Value) -> Option<String> {
    const TAIL_BYTES: u64 = 256 * 1024;

    let path = get_string(root, "transcript_path")?;
    let tail = read_file_tail(Path::new(path), TAIL_BYTES)?;

    let mut lines: Vec<&str> = tail.lines().collect();
    // 先頭行は途中で切れている可能性があるので捨てる。
    if tail.len() as u64 >= TAIL_BYTES && !lines.is_empty() {
        lines.remove(0);
    }

    for line in lines.iter().rev() {
        if !line.contains("\"effort\"") {
            continue;
        }

        let Ok(entry) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if entry.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }

        let effort = entry.get("effort")?;
        let level = effort
            .as_str()
            .or_else(|| effort.as_object().and_then(|_| get_string(effort, "level")));
        if let Some(level) = level.filter(|value| !is_blank(value)) {
            return Some(clean_text(level.trim()));
        }
    }

    None
}

/// ファイル末尾から最大 `limit` バイトを読む。会話ログは数メガバイトになるので、
/// 毎秒の再描画で全体を読まないようにする。
fn read_file_tail(path: &Path, limit: u64) -> Option<String> {
    use std::io::{Seek, SeekFrom};

    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(limit))).ok()?;

    let mut buffer = Vec::new();
    file.take(limit).read_to_end(&mut buffer).ok()?;
    Some(String::from_utf8_lossy(&buffer).into_owned())
}

fn get_session_cost(root: &Value) -> Option<f64> {
    get_object(root, "cost")
        .and_then(|cost| get_number(cost, "total_cost_usd"))
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn get_workspace_worktree(root: &Value) -> Option<String> {
    let value = get_object(root, "workspace").and_then(|workspace| get_string(workspace, "git_worktree"))?;
    if is_blank(value) {
        None
    } else {
        Some(clean_text(value.trim()))
    }
}

// ----------------------------------------------------------------- rate limits

fn get_rate_limits(root: &Value) -> RateLimits {
    let rate_limits = get_object(root, "rate_limits");
    let five_hour = rate_limits.and_then(|limits| get_object(limits, "five_hour"));
    let seven_day = rate_limits.and_then(|limits| get_object(limits, "seven_day"));
    let cached = if five_hour.is_none() || seven_day.is_none() {
        read_cached_rate_limits()
    } else {
        None
    };

    RateLimits {
        five_hour: five_hour.map_or_else(
            || cached.map(|cached| cached.five_hour).unwrap_or_default(),
            parse_rate_limit,
        ),
        seven_day: seven_day.map_or_else(
            || cached.map(|cached| cached.seven_day).unwrap_or_default(),
            parse_rate_limit,
        ),
    }
}

fn parse_rate_limit(limit: &Value) -> RateLimit {
    RateLimit {
        percentage: get_number(limit, "used_percentage").map(normalize_percentage),
        resets_at: limit.get("resets_at").and_then(parse_reset_time),
    }
}

fn parse_cached_rate_limit(limit: &Value) -> RateLimit {
    RateLimit {
        percentage: get_number(limit, "utilization").map(normalize_percentage),
        resets_at: limit.get("resets_at").and_then(parse_reset_time),
    }
}

fn read_cached_rate_limits() -> Option<RateLimits> {
    let path = PathBuf::from(std::env::var_os("HOME")?).join(".claude.json");
    let text = std::fs::read_to_string(path).ok()?;
    let document: Value = serde_json::from_str(&text).ok()?;
    let utilization = get_object(&document, "cachedUsageUtilization")
        .and_then(|cached| get_object(cached, "utilization"))?;

    Some(RateLimits {
        five_hour: get_object(utilization, "five_hour").map_or_else(RateLimit::default, parse_cached_rate_limit),
        seven_day: get_object(utilization, "seven_day").map_or_else(RateLimit::default, parse_cached_rate_limit),
    })
}

fn parse_reset_time(value: &Value) -> Option<DateTime<Utc>> {
    if let Some(seconds) = value.as_f64() {
        if seconds.is_finite() {
            return Utc.timestamp_opt(seconds.floor() as i64, 0).single();
        }
        return None;
    }

    let text = value.as_str()?.trim();
    if let Ok(epoch) = text.parse::<f64>() {
        if epoch.is_finite() {
            return Utc.timestamp_opt(epoch.floor() as i64, 0).single();
        }
        return None;
    }

    if let Ok(parsed) = DateTime::parse_from_rfc3339(text) {
        return Some(parsed.with_timezone(&Utc));
    }

    // Timestamps without an offset are assumed universal, matching
    // DateTimeStyles.AssumeUniversal on the C# side.
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(text, format) {
            return Some(Utc.from_utc_datetime(&naive));
        }
    }

    if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return Some(Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0)?));
    }

    None
}

fn create_rate_segment(label: &str, limit: RateLimit, scale: RateScale) -> Segment {
    let percentage = limit.percentage;
    let foreground = match scale {
        RateScale::Dark => WHITE,
        RateScale::Light => DARK_TEXT,
    };
    let background = get_rate_background(scale, percentage);
    let reset = format_reset(limit.resets_at);

    match percentage {
        None => Segment {
            background,
            foreground,
            text: format!(" {}: --%{} ", label, reset),
        },
        Some(value) => Segment {
            background,
            foreground,
            text: format!(
                " {} {} {}%{} ",
                label,
                build_usage_bar(value, foreground, background),
                format_percentage(value),
                reset
            ),
        },
    }
}

fn get_rate_background(scale: RateScale, percentage: Option<f64>) -> Color {
    let (low, medium, high) = match scale {
        RateScale::Dark => (
            RATE_DARK_LOW_BACKGROUND,
            RATE_DARK_MEDIUM_BACKGROUND,
            RATE_DARK_HIGH_BACKGROUND,
        ),
        RateScale::Light => (
            RATE_LIGHT_LOW_BACKGROUND,
            RATE_LIGHT_MEDIUM_BACKGROUND,
            RATE_LIGHT_HIGH_BACKGROUND,
        ),
    };

    match percentage {
        Some(value) if value >= 80.0 => high,
        Some(value) if value >= 50.0 => medium,
        _ => low,
    }
}

fn format_reset(reset: Option<DateTime<Utc>>) -> String {
    let Some(reset) = reset else {
        return String::new();
    };

    let local = reset.with_timezone(&Local);
    let remaining = local.signed_duration_since(Local::now());

    // すでに過ぎている場合、残り時間のカウントダウンは意味を持たない。しかし
    // ここで何も出さないと、上限に達して利用率の更新が止まったときに、ちょうど
    // リセット時刻まで消えてしまう。一番知りたいときに何も分からなくなるので、
    // 時刻そのものは残し、カウントダウンの代わりに `--` を置いて古い値だと示す。
    if remaining <= chrono::Duration::zero() {
        return if -remaining < chrono::Duration::days(1) {
            format!(" {:02}:{:02}(--)", local.hour(), local.minute())
        } else {
            format!(" {}/{}(--)", local.month(), local.day())
        };
    }

    if remaining < chrono::Duration::days(1) {
        return format!(
            " {:02}:{:02}({}h{}m)",
            local.hour(),
            local.minute(),
            remaining.num_hours(),
            remaining.num_minutes() % 60
        );
    }

    format!(
        " {}/{}({}d{}h)",
        local.month(),
        local.day(),
        remaining.num_days(),
        remaining.num_hours() % 24
    )
}

// ----------------------------------------------------------------------- git

fn find_repository(start_directory: &Path, configured_worktree: Option<String>) -> Option<Repository> {
    let mut directory = Some(start_directory);
    while let Some(current) = directory {
        let dot_git = current.join(".git");
        if dot_git.is_dir() {
            return Some(Repository {
                branch: read_branch(&dot_git.join("HEAD")),
                worktree: None,
                is_unborn: !has_head_commit(&dot_git),
            });
        }

        if dot_git.is_file() {
            let Some(git_directory) = resolve_git_directory(&dot_git) else {
                return Some(Repository {
                    branch: None,
                    worktree: None,
                    is_unborn: false,
                });
            };

            let derived = get_linked_worktree_name(&git_directory);
            let worktree = match configured_worktree {
                Some(ref value) if !is_blank(value) => Some(value.clone()),
                _ => derived,
            };

            return Some(Repository {
                branch: read_branch(&git_directory.join("HEAD")),
                worktree,
                is_unborn: !has_head_commit(&git_directory),
            });
        }

        directory = current.parent();
    }

    None
}

fn get_linked_worktree_name(git_directory: &Path) -> Option<String> {
    let name = git_directory.file_name()?.to_string_lossy().into_owned();
    let parent = git_directory.parent()?.file_name()?.to_string_lossy().into_owned();
    if parent.eq_ignore_ascii_case("worktrees") {
        Some(name)
    } else {
        None
    }
}

fn resolve_git_directory(dot_git_file: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(dot_git_file).ok()?;
    let text = text.trim();
    const PREFIX: &str = "gitdir:";
    if text.len() < PREFIX.len() || !text[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
        return None;
    }

    let path = text[PREFIX.len()..].trim();
    if path.is_empty() {
        return None;
    }

    let resolved = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        normalize(&dot_git_file.parent()?.join(path))
    };

    if resolved.is_dir() {
        Some(resolved)
    } else {
        None
    }
}

/// Lexical `.`/`..` collapse for an already-absolute path.
fn normalize(path: &Path) -> PathBuf {
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => resolved = PathBuf::from(prefix.as_os_str()),
            Component::RootDir => resolved.push("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(part) => resolved.push(part),
        }
    }

    resolved
}

fn read_branch(head_path: &Path) -> Option<String> {
    let head = std::fs::read_to_string(head_path).ok()?;
    let head = head.trim();
    const PREFIX: &str = "ref: refs/heads/";
    if let Some(branch) = head.strip_prefix(PREFIX) {
        let branch = branch.trim();
        return if branch.is_empty() {
            None
        } else {
            Some(branch.to_string())
        };
    }

    if head.is_empty() {
        None
    } else {
        Some(head.chars().take(8).collect())
    }
}

fn has_head_commit(git_directory: &Path) -> bool {
    let Ok(head) = std::fs::read_to_string(git_directory.join("HEAD")) else {
        return true;
    };

    let head = head.trim();
    let Some(reference) = head.strip_prefix("ref: ") else {
        return !head.is_empty();
    };

    let reference = reference.trim();
    if reference.is_empty() {
        return false;
    }

    // A linked worktree stores HEAD locally but refs and packed-refs in its
    // common Git directory, named by the local commondir file.
    let common_directory = get_common_git_directory(git_directory);
    let loose_reference = common_directory.join(reference);
    if let Ok(contents) = std::fs::read_to_string(&loose_reference) {
        if !is_blank(&contents) {
            return true;
        }
    }

    let packed_references = common_directory.join("packed-refs");
    let Ok(contents) = std::fs::read_to_string(packed_references) else {
        return false;
    };

    let suffix = format!(" {}", reference);
    contents
        .lines()
        .any(|line| !line.starts_with('#') && !line.starts_with('^') && line.ends_with(&suffix))
}

fn get_common_git_directory(git_directory: &Path) -> PathBuf {
    let Ok(common) = std::fs::read_to_string(git_directory.join("commondir")) else {
        return git_directory.to_path_buf();
    };

    let common = common.trim();
    if common.is_empty() {
        return git_directory.to_path_buf();
    }

    let resolved = if Path::new(common).is_absolute() {
        PathBuf::from(common)
    } else {
        normalize(&git_directory.join(common))
    };

    if resolved.is_dir() {
        resolved
    } else {
        git_directory.to_path_buf()
    }
}

fn get_diff_stat(repository: &Repository, directory: &Path) -> Option<(i64, i64)> {
    let stdout = run_git_diff(directory, repository.is_unborn)?;

    let mut added = 0i64;
    let mut deleted = 0i64;
    for line in stdout.split('\n').filter(|line| !line.is_empty()) {
        let mut fields = line.split('\t');
        let (Some(first), Some(second)) = (fields.next(), fields.next()) else {
            continue;
        };
        if first == "-" || second == "-" {
            continue;
        }

        if let Ok(value) = first.parse::<i64>() {
            added += value;
        }

        if let Ok(value) = second.parse::<i64>() {
            deleted += value;
        }
    }

    Some((added, deleted))
}

fn run_git_diff(directory: &Path, is_unborn: bool) -> Option<String> {
    let mut command = Command::new("git");
    command
        .arg("-c")
        .arg("core.fsmonitor=false")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(directory)
        .arg("diff")
        .arg("--numstat");
    if is_unborn {
        command.arg("--cached").arg(EMPTY_TREE);
    } else {
        command.arg("HEAD");
    }
    command
        .arg("--")
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;

    // Drain both pipes concurrently so a large diff cannot deadlock the wait.
    let stdout_reader = thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout.read_to_end(&mut buffer);
        buffer
    });
    let stderr_reader = thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stderr.read_to_end(&mut buffer);
        buffer
    });

    let deadline = Instant::now() + Duration::from_secs(3);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                thread::sleep(Duration::from_millis(2));
            }
            Err(_) => return None,
        }
    };

    let stdout = stdout_reader.join().ok()?;
    let _ = stderr_reader.join();
    if !status.success() {
        return None;
    }

    Some(String::from_utf8_lossy(&stdout).into_owned())
}

// ------------------------------------------------------------------ rendering

fn render_powerline(segments: &[Segment]) -> String {
    if segments.is_empty() {
        return String::new();
    }

    let mut output = String::new();
    for (index, segment) in segments.iter().enumerate() {
        if index == 0 {
            output.push_str(&background(segment.background));
        } else {
            output.push_str(&foreground(segments[index - 1].background));
            output.push_str(&background(segment.background));
            output.push(POWERLINE_RIGHT);
        }

        output.push_str(&foreground(segment.foreground));
        output.push_str(&segment.text);
    }

    let final_background = segments[segments.len() - 1].background;
    output.push_str(&foreground(final_background));
    output.push_str(ESCAPE);
    output.push_str("49m");
    output.push(POWERLINE_RIGHT);
    output.push_str(ESCAPE);
    output.push_str("0m");
    output
}

fn foreground(color: Color) -> String {
    format!("{}38;2;{};{};{}m", ESCAPE, color.red, color.green, color.blue)
}

fn background(color: Color) -> String {
    format!("{}48;2;{};{};{}m", ESCAPE, color.red, color.green, color.blue)
}

// ------------------------------------------------------------------ subagents

fn write_subagent_status(root: &Value) {
    let Some(tasks) = root.get("tasks").and_then(Value::as_array) else {
        return;
    };

    let session_effort = get_effort(root).or_else(|| get_transcript_effort(root));
    let columns = get_number(root, "columns").filter(|value| *value > 0.0).map(|value| value as i64);
    let now_ms = current_time_millis();

    for task in tasks {
        if !task.is_object() {
            continue;
        }

        let Some(id) = get_string(task, "id").filter(|value| !value.is_empty()) else {
            continue;
        };

        if let Some(content) = build_subagent_content(task, session_effort.as_deref(), columns, now_ms) {
            println!(
                "{{\"id\":{},\"content\":{}}}",
                Value::String(id.to_string()),
                Value::String(content)
            );
        }
    }
}

fn current_time_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// サブエージェント1行は、メインの statusline と同じ Powerline セグメントで
/// 組む。順序は モデル→Effort→コンテキスト→進捗→エージェント→ラベル で固定。
/// 既知のEffort には本体と同じ段数ゲージを付ける。コンテキストの使用率バーは
/// 細いステータスパネルで幅を使いすぎるため省く。
fn build_subagent_content(
    task: &Value,
    session_effort: Option<&str>,
    columns: Option<i64>,
    now_ms: i64,
) -> Option<String> {
    let model = get_string(task, "model")
        .filter(|value| !is_blank(value))
        .map(|model| Segment {
            background: MODEL_BACKGROUND,
            foreground: WHITE,
            text: format!(" {} ", prettify_model(&clean_text(model))),
        });

    // Effort セグメントの中身は本体の statusline と同じ `create_effort_text` に
    // 任せる。既知の5段階ならゲージを添え、`EFFORT_BACKGROUND` を囲みの色として
    // 渡すのでセグメント内で完結する。budget（数値のトークン予算）や未知の値は
    // 段が定まらずゲージを出しようがないので、ラベルだけになる。
    // ラベル文字列は削減カスケードでゲージ無しに作り直すために取っておく。
    let effort_label = get_task_effort(task, session_effort);
    let mut effort = effort_label.as_deref().map(|label| Segment {
        background: EFFORT_BACKGROUND,
        foreground: WHITE,
        text: create_effort_text(label),
    });

    // コンテキストは常に出す。Effort と進捗のあいだに必ず挟まる面が無いと、
    // `tokenCount` が欠けた入力では Effort と進捗が直接隣り合ってしまい、
    // 両者の紫は色差 15.5 しかなく境界が見えなくなる（`adjacent_background_pairs`
    // 参照）。取得できない場合はメインの statusline の `--%` 表示にならい、
    // トークン数も使用率も無いことを ` -- ` で示す。
    let token_count = get_number(task, "tokenCount");
    let context = Some(match token_count {
        Some(token_count) => build_context_segment(token_count, get_number(task, "contextWindowSize")),
        None => Segment {
            background: get_context_background(None),
            foreground: DARK_TEXT,
            text: " -- ".to_string(),
        },
    });

    let mut elapsed = format_elapsed(get_number(task, "startTime"), now_ms);
    let mut sparkline = build_sparkline(&get_number_array(task, "tokenSamples"));
    let mut agent = get_agent_label(task).map(|label| Segment {
        background: IDENTITY_BACKGROUND,
        foreground: WHITE,
        text: format!(" {} ", label),
    });
    let mut label = get_string(task, "label")
        .filter(|value| !is_blank(value))
        .or_else(|| get_string(task, "description").filter(|value| !is_blank(value)))
        .map(clean_text)
        .filter(|value| !is_blank(value));

    let mut progress = build_progress_segment(elapsed.as_deref(), sparkline.as_deref());

    // コンテキストは上で常に `Some` にしているので、ここでは元データの有無
    // （`tokenCount` が取れたかどうか）で判定する。
    if model.is_none()
        && effort.is_none()
        && token_count.is_none()
        && progress.is_none()
        && agent.is_none()
        && label.is_none()
    {
        return None;
    }

    let mut label_segment = build_label_segment(label.as_deref());
    let budget = columns.unwrap_or(60) - 4;

    // 予算を超える場合はユーザー承認済みの順で削る:
    // ゲージ → ラベルの切り詰め/削除 → スパークライン → エージェント →
    // 経過時間（結果として進捗セグメントが空になれば削除） → Effort。
    // モデルとコンテキストは常に残す。

    // ゲージを最初に落とすのは、段階名という情報そのものは残したまま
    // 一度に11桁空くからで、ラベルを削るより失うものが小さい。
    if subagent_width(&[&model, &effort, &context, &progress, &agent, &label_segment]) > budget {
        if let (Some(segment), Some(label)) = (effort.as_mut(), effort_label.as_deref()) {
            segment.text = format!(" {} ", label);
        }
    }

    if subagent_width(&[&model, &effort, &context, &progress, &agent, &label_segment]) > budget {
        let other_width = subagent_width(&[&model, &effort, &context, &progress, &agent]);
        label = truncate_or_drop_label(label, other_width, budget);
        label_segment = build_label_segment(label.as_deref());
    }

    if subagent_width(&[&model, &effort, &context, &progress, &agent, &label_segment]) > budget {
        sparkline = None;
        progress = build_progress_segment(elapsed.as_deref(), sparkline.as_deref());
    }

    if subagent_width(&[&model, &effort, &context, &progress, &agent, &label_segment]) > budget {
        agent = None;
    }

    if subagent_width(&[&model, &effort, &context, &progress, &agent, &label_segment]) > budget {
        elapsed = None;
        progress = build_progress_segment(elapsed.as_deref(), sparkline.as_deref());
    }

    if subagent_width(&[&model, &effort, &context, &progress, &agent, &label_segment]) > budget {
        effort = None;
    }

    let segments: Vec<Segment> = [model, effort, context, progress, agent, label_segment]
        .into_iter()
        .flatten()
        .collect();

    if segments.is_empty() {
        None
    } else {
        Some(render_powerline(&segments))
    }
}

/// Powerline の実表示幅は「各セグメントの表示幅の合計＋セグメント数」
/// （セグメント間の楔 n-1 個＋末尾の楔1個）。ANSIエスケープは幅に数えない。
/// 説明・ラベルには日本語が入りうるため、文字数ではなく表示幅で測る。
fn subagent_width(pieces: &[&Option<Segment>]) -> i64 {
    let mut text_len = 0i64;
    let mut count = 0i64;
    for piece in pieces {
        if let Some(segment) = piece {
            text_len += display_width(&strip_ansi(&segment.text));
            count += 1;
        }
    }
    text_len + count
}

/// ラベルは「他の全セグメント＋自分の楔＋前後の空白」を差し引いた残りに
/// 収める。収まらないなら `...`（表示幅3）を付けて切り詰め、10桁未満しか
/// 残らないなら（自明すぎて読めないので）セグメントごと落とす。
fn truncate_or_drop_label(label: Option<String>, other_width: i64, budget: i64) -> Option<String> {
    let label = label?;
    let allowed_content = budget - other_width - 1 - 2;
    if allowed_content < 10 {
        return None;
    }

    if display_width(&label) <= allowed_content {
        return Some(label);
    }

    Some(truncate_to_width(&label, allowed_content - 3) + "...")
}

/// 結合文字は表示幅0、東アジアの全角文字は表示幅2、それ以外は1として数える。
/// 依存クレートを増やさないための最小限の自前実装。
fn char_display_width(character: char) -> i64 {
    let code = character as u32;

    // 結合文字（結合分音記号）は単独では桁を持たない。
    if (0x0300..=0x036F).contains(&code) {
        return 0;
    }

    // East Asian Wide / Fullwidth の主要な範囲。U+2581–2588（スパークライン）
    // は East Asian Ambiguous だがどの範囲にも含まれず、意図どおり1桁のまま
    // 扱われる。
    const WIDE_RANGES: [(u32, u32); 16] = [
        (0x1100, 0x115F),
        (0x2E80, 0x303E),
        (0x3041, 0x33FF),
        (0x3400, 0x4DBF),
        (0x4E00, 0x9FFF),
        (0xA000, 0xA4CF),
        (0xAC00, 0xD7A3),
        (0xF900, 0xFAFF),
        (0xFE10, 0xFE19),
        (0xFE30, 0xFE6F),
        (0xFF00, 0xFF60),
        (0xFFE0, 0xFFE6),
        (0x1F300, 0x1F64F),
        (0x1F900, 0x1F9FF),
        (0x20000, 0x2FFFD),
        (0x30000, 0x3FFFD),
    ];

    if WIDE_RANGES.iter().any(|&(start, end)| (start..=end).contains(&code)) {
        2
    } else {
        1
    }
}

fn display_width(text: &str) -> i64 {
    text.chars().map(char_display_width).sum()
}

/// ANSI SGR エスケープ（`\x1b[...m`）を取り除き、目に見える文字だけ残す。
fn strip_ansi(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next == 'm' {
                    break;
                }
            }
            continue;
        }
        result.push(character);
    }
    result
}

/// 表示幅が `max_width` に収まるところまで文字を残す。全角文字を割って
/// 半端な1桁を残すことがないよう、次の1文字を足すと超える時点で止める。
fn truncate_to_width(text: &str, max_width: i64) -> String {
    let mut result = String::new();
    let mut width = 0i64;
    for character in text.chars() {
        let character_width = char_display_width(character);
        if width + character_width > max_width {
            break;
        }
        width += character_width;
        result.push(character);
    }
    result
}

fn build_context_segment(token_count: f64, context_window_size: Option<f64>) -> Segment {
    match context_window_size.filter(|value| *value > 0.0) {
        Some(context_size) => {
            let displayed = round_half_away_from_zero(token_count / context_size * 100.0);
            Segment {
                background: get_context_background(Some(displayed)),
                foreground: DARK_TEXT,
                text: format!(
                    " {}/{} {}% ",
                    format_rounded_k(token_count),
                    format_rounded_context(context_size),
                    format_integer(displayed)
                ),
                }
        }
        // 上限が無ければ使用率は出しようがないので、トークン数だけ見せる。
        None => Segment {
            background: get_context_background(None),
            foreground: DARK_TEXT,
            text: format!(" {} ", format_rounded_k(token_count)),
        },
    }
}

fn build_progress_segment(elapsed: Option<&str>, sparkline: Option<&str>) -> Option<Segment> {
    let mut parts: Vec<&str> = Vec::new();
    if let Some(elapsed) = elapsed {
        parts.push(elapsed);
    }
    if let Some(sparkline) = sparkline {
        parts.push(sparkline);
    }

    if parts.is_empty() {
        return None;
    }

    Some(Segment {
        background: WORKTREE_BACKGROUND,
        foreground: WHITE,
        text: format!(" {} ", parts.join(" ")),
    })
}

fn build_label_segment(label: Option<&str>) -> Option<Segment> {
    label.map(|label| Segment {
        background: COST_BACKGROUND,
        foreground: DARK_TEXT,
        text: format!(" {} ", label),
    })
}

/// `startTime` はエポック秒・ミリ秒のどちらでも渡されうる。現実的な日時なら
/// 秒表記は13桁未満、ミリ秒表記は13桁以上になるので、1e12 を境に振り分けて
/// 正規化する。
fn normalize_start_time_ms(value: f64) -> f64 {
    if value.abs() < 1e12 {
        value * 1000.0
    } else {
        value
    }
}

/// 経過時間の表示。`startTime` が未来・欠落・不正なら None を返し、
/// 進捗セグメントからその部分だけを省く。
fn format_elapsed(start_time: Option<f64>, now_ms: i64) -> Option<String> {
    let start_time = start_time.filter(|value| value.is_finite())?;
    let elapsed_ms = now_ms as f64 - normalize_start_time_ms(start_time);
    if elapsed_ms < 0.0 {
        return None;
    }

    Some(format_duration((elapsed_ms / 1000.0) as i64))
}

/// 2単位までの経過表示。`1h02m` / `2m14s` / `47s`。
fn format_duration(total_seconds: i64) -> String {
    let total_seconds = total_seconds.max(0);
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    if hours > 0 {
        format!("{}h{:02}m", hours, minutes)
    } else if minutes > 0 {
        format!("{}m{:02}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}

const SPARKLINE_LEVELS: [char; 8] =
    ['\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}', '\u{2588}'];

/// `tokenSamples` にはタイムスタンプが無いので、毎秒のレートは絶対に出せない。
/// 隣接差分だけを8段階に量子化する。差分の最大値を上限に正規化し、
/// 全差分が0（またはサンプルが減少方向のみ）なら最低段を並べる。
fn build_sparkline(samples: &[f64]) -> Option<String> {
    if samples.len() < 2 {
        return None;
    }

    let diffs: Vec<f64> = samples.windows(2).map(|pair| pair[1] - pair[0]).collect();
    let max_diff = diffs.iter().copied().fold(0.0, f64::max);

    let mut sparkline = String::new();
    for diff in diffs {
        let index = if max_diff <= 0.0 {
            0
        } else {
            let ratio = (diff / max_diff).clamp(0.0, 1.0);
            (ratio * (SPARKLINE_LEVELS.len() - 1) as f64).round() as usize
        };
        sparkline.push(SPARKLINE_LEVELS[index.min(SPARKLINE_LEVELS.len() - 1)]);
    }

    Some(sparkline)
}

fn get_number_array(element: &Value, property_name: &str) -> Vec<f64> {
    element
        .as_object()
        .and_then(|object| object.get(property_name))
        .and_then(Value::as_array)
        .map(|array| array.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default()
}

/// `name` があればそれを使う。無ければ `type` を短縮ラベルへ変換するが、
/// `local_agent` は名乗りようがないので省略する。未知の `type` も安全側で省略。
fn get_agent_label(task: &Value) -> Option<String> {
    if let Some(name) = get_string(task, "name").filter(|value| !is_blank(value)) {
        return Some(clean_text(name));
    }

    match get_string(task, "type")? {
        "local_bash" => Some("bash".to_string()),
        "local_workflow" => Some("workflow".to_string()),
        "remote_agent" => Some("remote".to_string()),
        "in_process_teammate" => Some("teammate".to_string()),
        _ => None,
    }
}

fn get_task_effort(task: &Value, session_effort: Option<&str>) -> Option<String> {
    if let Some(effort) = task.get("effort") {
        if let Some(value) = effort.as_str() {
            return if value.is_empty() {
                None
            } else {
                Some(capitalize(&clean_text(value)))
            };
        }

        if let Some(budget) = effort.as_f64() {
            return Some(format_rounded_k(budget));
        }
    }

    session_effort.map(|effort| capitalize(&clean_text(effort)))
}

fn get_effort(root: &Value) -> Option<String> {
    let effort = root.get("effort")?;
    if let Some(value) = effort.as_str() {
        return Some(clean_text(value));
    }

    if effort.is_object() {
        return get_string(effort, "level").map(clean_text);
    }

    None
}

fn prettify_model(model: &str) -> String {
    match model {
        "claude-fable-5" => "Fable 5".to_string(),
        "claude-opus-5" => "Opus 5".to_string(),
        "claude-sonnet-5" => "Sonnet 5".to_string(),
        _ if model.starts_with("claude-haiku-4-5") => "Haiku 4.5".to_string(),
        _ => prettify_model_fallback(model),
    }
}

fn prettify_model_fallback(model: &str) -> String {
    let text = model.strip_prefix("claude-").unwrap_or(model);
    let text = strip_date_suffix(text);
    let parts: Vec<&str> = text.split('-').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return model.to_string();
    }

    let mut rendered: Vec<String> = parts.iter().map(|part| (*part).to_string()).collect();
    rendered[0] = capitalize(&rendered[0]);
    rendered.join(" ")
}

/// Equivalent of the `-\d{6,8}$` trim in the C# implementation.
fn strip_date_suffix(text: &str) -> &str {
    let Some(index) = text.rfind('-') else {
        return text;
    };

    let suffix = &text[index + 1..];
    if (6..=8).contains(&suffix.len()) && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
        &text[..index]
    } else {
        text
    }
}

// -------------------------------------------------------------------- helpers

fn capitalize(value: &str) -> String {
    let mut characters = value.chars();
    match characters.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
    }
}

fn clean_text(value: &str) -> String {
    value
        .chars()
        .map(|character| if character.is_control() { ' ' } else { character })
        .collect()
}

fn is_blank(value: &str) -> bool {
    value.trim().is_empty()
}

fn get_object<'a>(element: &'a Value, property_name: &str) -> Option<&'a Value> {
    element
        .as_object()?
        .get(property_name)
        .filter(|value| value.is_object())
}

fn get_string<'a>(element: &'a Value, property_name: &str) -> Option<&'a str> {
    element.as_object()?.get(property_name)?.as_str()
}

fn get_number(element: &Value, property_name: &str) -> Option<f64> {
    element.as_object()?.get(property_name)?.as_f64()
}

// ---------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    const CONTEXT_BANDS: [Color; 4] = [
        CONTEXT_LOW_BACKGROUND,
        CONTEXT_MEDIUM_BACKGROUND,
        CONTEXT_HIGH_BACKGROUND,
        CONTEXT_CRITICAL_BACKGROUND,
    ];
    const RATE_DARK_BANDS: [Color; 3] = [
        RATE_DARK_LOW_BACKGROUND,
        RATE_DARK_MEDIUM_BACKGROUND,
        RATE_DARK_HIGH_BACKGROUND,
    ];
    const RATE_LIGHT_BANDS: [Color; 3] = [
        RATE_LIGHT_LOW_BACKGROUND,
        RATE_LIGHT_MEDIUM_BACKGROUND,
        RATE_LIGHT_HIGH_BACKGROUND,
    ];

    fn to_linear(channel: i32) -> f64 {
        let channel = f64::from(channel.clamp(0, 255)) / 255.0;
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    }

    /// sRGB を CIE L*a*b*（D65）へ変換する。
    fn to_lab(color: Color) -> (f64, f64, f64) {
        let (red, green, blue) = (
            to_linear(color.red),
            to_linear(color.green),
            to_linear(color.blue),
        );
        let x = (red * 0.4124564 + green * 0.3575761 + blue * 0.1804375) / 0.95047;
        let y = red * 0.2126729 + green * 0.7151522 + blue * 0.0721750;
        let z = (red * 0.0193339 + green * 0.1191920 + blue * 0.9503041) / 1.08883;

        let f = |t: f64| {
            if t > 216.0 / 24389.0 {
                t.cbrt()
            } else {
                (841.0 / 108.0) * t + 4.0 / 29.0
            }
        };
        let (fx, fy, fz) = (f(x), f(y), f(z));
        (116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz))
    }

    /// CIE76 の色差。2.3 前後でようやく違いが分かり、10 を超えれば別の色に見える。
    fn delta_e(first: Color, second: Color) -> f64 {
        let (l1, a1, b1) = to_lab(first);
        let (l2, a2, b2) = to_lab(second);
        ((l1 - l2).powi(2) + (a1 - a2).powi(2) + (b1 - b2).powi(2)).sqrt()
    }

    fn relative_luminance(color: Color) -> f64 {
        0.2126 * to_linear(color.red)
            + 0.7152 * to_linear(color.green)
            + 0.0722 * to_linear(color.blue)
    }

    fn contrast_ratio(first: Color, second: Color) -> f64 {
        let (first, second) = (relative_luminance(first), relative_luminance(second));
        let (higher, lower) = if first >= second {
            (first, second)
        } else {
            (second, first)
        };
        (higher + 0.05) / (lower + 0.05)
    }

    /// 隣り合いうる背景色の全組み合わせ。ワークツリーのセグメントは省略される
    /// ことがあるので、ブランチ→差分の直結も含める。
    fn adjacent_background_pairs() -> Vec<(String, Color, Color)> {
        let mut pairs = vec![
            ("identity → directory".to_string(), IDENTITY_BACKGROUND, DIRECTORY_BACKGROUND),
            ("directory → model".to_string(), DIRECTORY_BACKGROUND, MODEL_BACKGROUND),
            ("model → effort".to_string(), MODEL_BACKGROUND, EFFORT_BACKGROUND),
            ("cost → git".to_string(), COST_BACKGROUND, GIT_BACKGROUND),
            ("cost → diff".to_string(), COST_BACKGROUND, DIFF_BACKGROUND),
            ("git → worktree".to_string(), GIT_BACKGROUND, WORKTREE_BACKGROUND),
            ("git → diff".to_string(), GIT_BACKGROUND, DIFF_BACKGROUND),
            ("worktree → diff".to_string(), WORKTREE_BACKGROUND, DIFF_BACKGROUND),
        ];

        for (index, context) in CONTEXT_BANDS.into_iter().enumerate() {
            pairs.push((format!("effort → context[{index}]"), EFFORT_BACKGROUND, context));
            for (rate, dark) in RATE_DARK_BANDS.into_iter().enumerate() {
                pairs.push((format!("context[{index}] → 5h[{rate}]"), context, dark));
            }
        }

        for (index, dark) in RATE_DARK_BANDS.into_iter().enumerate() {
            for (rate, light) in RATE_LIGHT_BANDS.into_iter().enumerate() {
                pairs.push((format!("5h[{index}] → 7d[{rate}]"), dark, light));
            }
        }

        for (index, light) in RATE_LIGHT_BANDS.into_iter().enumerate() {
            // コストは入力に含まれないことがあるので、7d の次はどちらもありうる。
            pairs.push((format!("7d[{index}] → git"), light, GIT_BACKGROUND));
            pairs.push((format!("7d[{index}] → cost"), light, COST_BACKGROUND));
        }

        // --- サブエージェント行（モデル→Effort→コンテキスト→進捗→エージェント→ラベル） ---
        // モデルは省略されうるが、コンテキストは `build_subagent_content` が
        // `tokenCount` の有無にかかわらず必ず1つ出す（取れなければ ` -- `）。
        // Effort・進捗・エージェント・ラベルは省略されうるので、コンテキストを
        // 飛び越した隣接（モデル→コンテキスト、コンテキスト→エージェント／
        // ラベル）が生じる。一方 Effort は必ずコンテキストの直前にしか置かれ
        // ないため、コンテキストを飛び越して進捗以降に隣り合うことはない
        // （両方とも紫系で、隣り合うと色差15.5しかなく境界が消える）。
        // `model → effort` と `effort → context[*]` は上のメイン行と同じ
        // 色の組み合わせなので、そちらのテストで既にカバーされている。
        for (index, context) in CONTEXT_BANDS.into_iter().enumerate() {
            pairs.push((format!("model → context[{index}]"), MODEL_BACKGROUND, context));
            pairs.push((format!("context[{index}] → progress"), context, WORKTREE_BACKGROUND));
            pairs.push((format!("context[{index}] → agent"), context, IDENTITY_BACKGROUND));
            pairs.push((format!("context[{index}] → label"), context, COST_BACKGROUND));
        }
        pairs.push(("progress → agent".to_string(), WORKTREE_BACKGROUND, IDENTITY_BACKGROUND));
        pairs.push(("progress → label".to_string(), WORKTREE_BACKGROUND, COST_BACKGROUND));
        pairs.push(("agent → label".to_string(), IDENTITY_BACKGROUND, COST_BACKGROUND));

        pairs
    }

    /// 区切りは前のセグメントの背景色で描かれるため、隣り合う背景が近いと
    /// 境界そのものが消える。WCAG のコントラスト比は輝度しか見ず色相差を
    /// 無視する（緑と紫が「同じ」と判定されてしまう）ので、色差で検証する。
    #[test]
    fn adjacent_backgrounds_are_perceptually_distinct() {
        for (label, first, second) in adjacent_background_pairs() {
            let difference = delta_e(first, second);
            assert!(
                difference >= 20.0,
                "{label}: 色差 {difference:.1} は近すぎて区切りが見えない"
            );
        }
    }

    /// 文字は WCAG AA（4.5）を満たすこと。中間輝度の背景は白でも黒でも
    /// 比が上がらないので、この検証を通らない背景色は採用できない。
    #[test]
    fn segment_text_meets_wcag_aa() {
        let mut pairs = vec![
            ("identity", IDENTITY_BACKGROUND, WHITE),
            ("directory", DIRECTORY_BACKGROUND, WHITE),
            ("model", MODEL_BACKGROUND, WHITE),
            ("effort", EFFORT_BACKGROUND, WHITE),
            ("git", GIT_BACKGROUND, WHITE),
            ("worktree", WORKTREE_BACKGROUND, WHITE),
            ("diff", DIFF_BACKGROUND, DARK_TEXT),
            ("cost", COST_BACKGROUND, DARK_TEXT),
            // サブエージェント行の進捗・エージェント・ラベルは上と同じ配色を
            // 再利用しているだけだが、その事実を明示するために別名でも足す。
            ("subagent progress", WORKTREE_BACKGROUND, WHITE),
            ("subagent agent", IDENTITY_BACKGROUND, WHITE),
            ("subagent label", COST_BACKGROUND, DARK_TEXT),
        ];
        for context in CONTEXT_BANDS {
            pairs.push(("context", context, DARK_TEXT));
        }
        for dark in RATE_DARK_BANDS {
            pairs.push(("5h", dark, WHITE));
        }
        for light in RATE_LIGHT_BANDS {
            pairs.push(("7d", light, DARK_TEXT));
        }

        for (label, background, text) in pairs {
            let ratio = contrast_ratio(background, text);
            assert!(ratio >= 4.5, "{label}: コントラスト比 {ratio:.2} は AA 未満");
        }
    }

    /// 既知の各段階は、上の段も黒い空セルとして残した5セルのゲージになること。
    #[test]
    fn effort_gauge_always_shows_five_cells() {
        for step in 1..=EFFORT_STEPS {
            for phase in 0..step + 1 + EFFORT_HOLD_FRAMES {
                let gauge = build_effort_gauge(step, phase, Some(EFFORT_BACKGROUND));
                let ordinary = gauge.matches(POWERLINE_RIGHT).count();
                let thin = gauge.matches(POWERLINE_RIGHT_THIN).count();

                assert_eq!(gauge.matches(' ').count(), EFFORT_STEPS, "step {step} phase {phase}: セル数");
                assert_eq!(ordinary + thin, EFFORT_STEPS + 1, "step {step} phase {phase}: 境界の数");
                assert_eq!(
                    thin,
                    EFFORT_STEPS.saturating_sub(step + 1),
                    "step {step} phase {phase}: 同色の空セル境界"
                );
                assert!(
                    !gauge.contains('\u{2591}'),
                    "step {step} phase {phase}: 空セルに点字プレースホルダーがある"
                );
                assert_eq!(
                    gauge.matches(&background(EFFORT_EMPTY_BACKGROUND)).count(),
                    EFFORT_STEPS - step,
                    "step {step} phase {phase}: 黒い空セルの数"
                );
            }
        }
    }

    /// High は紫の3セルと、文字を置かない黒い2セルを持つ。黒セルどうしは
    /// 同じ黒地のまま細い楔で区切り、最後はEffort地へ正しく戻ること。
    #[test]
    fn high_effort_has_three_coloured_and_two_black_blank_cells() {
        let gauge = build_effort_gauge(3, 3, Some(EFFORT_BACKGROUND));
        let expected = format!(
            "{}{}{} {}{}{} {}{}{} {}{}{} {}{}{} {}{}{}{}",
            foreground(EFFORT_BACKGROUND),
            background(EFFORT_LIT_BACKGROUNDS[0]),
            POWERLINE_RIGHT,
            foreground(EFFORT_LIT_BACKGROUNDS[0]),
            background(EFFORT_LIT_BACKGROUNDS[1]),
            POWERLINE_RIGHT,
            foreground(EFFORT_LIT_BACKGROUNDS[1]),
            background(EFFORT_LIT_BACKGROUNDS[2]),
            POWERLINE_RIGHT,
            foreground(EFFORT_LIT_BACKGROUNDS[2]),
            background(EFFORT_EMPTY_BACKGROUND),
            POWERLINE_RIGHT,
            foreground(EFFORT_EMPTY_DIVIDER),
            background(EFFORT_EMPTY_BACKGROUND),
            POWERLINE_RIGHT_THIN,
            foreground(EFFORT_EMPTY_BACKGROUND),
            background(EFFORT_BACKGROUND),
            POWERLINE_RIGHT,
            foreground(WHITE),
        );

        assert_eq!(gauge, expected);
        assert_eq!(gauge.matches(' ').count(), 5);
        assert_eq!(gauge.matches(&background(EFFORT_EMPTY_BACKGROUND)).count(), 2);
        assert!(!gauge.contains('\u{2591}'));
    }

    /// アニメーションは現在の段階まで1段ずつ満ちていき、そこで止まること。
    /// 段階より上の段が点いてはならない。
    #[test]
    fn effort_gauge_fills_up_to_the_level_and_stops() {
        for step in 1..=EFFORT_STEPS {
            let cycle = step + 1 + EFFORT_HOLD_FRAMES;
            let lit_counts: Vec<usize> = (0..cycle)
                .map(|phase| {
                    let gauge = build_effort_gauge(step, phase, Some(EFFORT_BACKGROUND));
                    EFFORT_LIT_BACKGROUNDS
                        .into_iter()
                        .filter(|color| gauge.contains(&background(*color)))
                        .count()
                })
                .collect();

            let expected: Vec<usize> = (0..cycle).map(|phase| phase.min(step)).collect();
            assert_eq!(lit_counts, expected, "step {step}: 満ち方");

            assert_eq!(
                lit_counts.iter().filter(|count| **count == step).count(),
                EFFORT_HOLD_FRAMES + 1,
                "step {step}: 満ちきったあと留まるコマ数"
            );

            // 段階より上の段は、どのコマでも点灯色にならない。
            for phase in 0..cycle * 3 {
                let gauge = build_effort_gauge(step, phase, Some(EFFORT_BACKGROUND));
                for (above, color) in EFFORT_LIT_BACKGROUNDS.into_iter().enumerate().skip(step) {
                    assert!(
                        !gauge.contains(&background(color)),
                        "step {step} phase {phase}: 段 {above} が点いている"
                    );
                }
            }
        }
    }

    /// ゲージはセグメント本来の色へ戻して終わること。戻し忘れると外側の
    /// Powerline 接続の色がずれる。
    #[test]
    fn effort_gauge_restores_segment_colours() {
        let tail = format!(
            "{}{}{}",
            background(EFFORT_BACKGROUND),
            POWERLINE_RIGHT,
            foreground(WHITE)
        );
        for step in 1..=EFFORT_STEPS {
            for phase in 0..step + 1 + EFFORT_HOLD_FRAMES {
                assert!(
                    build_effort_gauge(step, phase, Some(EFFORT_BACKGROUND)).ends_with(&tail),
                    "step {step} phase {phase}"
                );
            }
        }
    }

    /// 待機色は点灯色と同じ明度で彩度だけを落としたもの。輝度が一致していれば、
    /// アニメーションのどのコマでも「段階の範囲内かどうか」が同じ強さで読める。
    #[test]
    fn effort_pending_matches_lit_luminance() {
        for index in 0..EFFORT_STEPS {
            let lit = relative_luminance(EFFORT_LIT_BACKGROUNDS[index]);
            let pending = relative_luminance(EFFORT_PENDING_BACKGROUNDS[index]);
            let ratio = if lit >= pending { lit / pending } else { pending / lit };
            assert!(ratio <= 1.05, "段 {index}: 点灯と待機の輝度が {ratio:.3} 倍ずれている");
        }
    }

    /// 点灯・待機のどちらも、範囲外の段とは輝度で分離していること。1マス幅の
    /// 図と地の判別なので、色差ではなく輝度比で決まる。
    #[test]
    fn effort_gauge_lit_steps_stand_out_from_unlit() {
        for index in 0..EFFORT_STEPS {
            for (label, color) in [
                ("点灯", EFFORT_LIT_BACKGROUNDS[index]),
                ("待機", EFFORT_PENDING_BACKGROUNDS[index]),
            ] {
                let ratio = contrast_ratio(color, EFFORT_EMPTY_BACKGROUND);
                assert!(ratio >= 3.5, "段 {index} の{label}: コントラスト比 {ratio:.2}");
            }
        }
    }

    /// 隣り合う段の境界が見えること。アニメーション中は点灯と待機が隣り合うので、
    /// 同じ状態どうしだけでなく状態をまたぐ組み合わせも確かめる。
    #[test]
    fn effort_gauge_steps_are_distinguishable() {
        for index in 0..EFFORT_STEPS - 1 {
            for (label, first, second) in [
                ("点灯→点灯", EFFORT_LIT_BACKGROUNDS[index], EFFORT_LIT_BACKGROUNDS[index + 1]),
                ("待機→待機", EFFORT_PENDING_BACKGROUNDS[index], EFFORT_PENDING_BACKGROUNDS[index + 1]),
                ("点灯→待機", EFFORT_LIT_BACKGROUNDS[index], EFFORT_PENDING_BACKGROUNDS[index + 1]),
                ("待機→点灯", EFFORT_PENDING_BACKGROUNDS[index], EFFORT_LIT_BACKGROUNDS[index + 1]),
            ] {
                let difference = delta_e(first, second);
                assert!(difference >= 8.0, "段 {index} {label}: 色差 {difference:.1}");
            }
        }

        // 水位（段階の内と外）と、ゲージの出入り。
        for index in 0..EFFORT_STEPS {
            for (label, color) in [
                ("点灯", EFFORT_LIT_BACKGROUNDS[index]),
                ("待機", EFFORT_PENDING_BACKGROUNDS[index]),
            ] {
                let difference = delta_e(color, EFFORT_EMPTY_BACKGROUND);
                assert!(difference >= 20.0, "段 {index} の{label}→範囲外: 色差 {difference:.1}");
            }
        }

        for (label, first, second) in [
            ("背景→点灯1", EFFORT_BACKGROUND, EFFORT_LIT_BACKGROUNDS[0]),
            ("背景→待機1", EFFORT_BACKGROUND, EFFORT_PENDING_BACKGROUNDS[0]),
            ("未点灯→背景", EFFORT_EMPTY_BACKGROUND, EFFORT_BACKGROUND),
            ("点灯5→背景", EFFORT_LIT_BACKGROUNDS[EFFORT_STEPS - 1], EFFORT_BACKGROUND),
            ("待機5→背景", EFFORT_PENDING_BACKGROUNDS[EFFORT_STEPS - 1], EFFORT_BACKGROUND),
        ] {
            let difference = delta_e(first, second);
            assert!(difference >= 8.0, "{label}: 色差 {difference:.1}");
        }

        let ending = contrast_ratio(EFFORT_EMPTY_BACKGROUND, EFFORT_BACKGROUND);
        assert!(ending >= 2.0, "未到達を示す終端のコントラスト比 {ending:.2}");

        let empty_divider = contrast_ratio(EFFORT_EMPTY_DIVIDER, EFFORT_EMPTY_BACKGROUND);
        assert!(
            empty_divider >= 2.0,
            "同色の空セルを区切る細い楔のコントラスト比 {empty_divider:.2}"
        );
    }

    /// 段が上がるほど明るくなること。点灯・待機のどちらの並びでも保つ。
    #[test]
    fn effort_gauge_brightens_with_each_step() {
        for (label, ramp) in [
            ("点灯", EFFORT_LIT_BACKGROUNDS),
            ("待機", EFFORT_PENDING_BACKGROUNDS),
        ] {
            for index in 0..EFFORT_STEPS - 1 {
                let lower = relative_luminance(ramp[index]);
                let higher = relative_luminance(ramp[index + 1]);
                assert!(higher > lower, "{label}: 段 {index} より次の段が明るくない");
            }
        }
    }

    /// 端末の地へ直接描く場合も、外側の先頭楔を除いて5セルを残すこと。
    #[test]
    fn effort_gauge_on_terminal_ground_always_has_five_cells() {
        for step in 1..=EFFORT_STEPS {
            for phase in 0..step + 1 + EFFORT_HOLD_FRAMES {
                let gauge = build_effort_gauge(step, phase, None);

                let separators = gauge.matches(POWERLINE_RIGHT).count()
                    + gauge.matches(POWERLINE_RIGHT_THIN).count();
                let bodies = gauge.matches(' ').count();
                assert_eq!(bodies, EFFORT_STEPS, "step {step} phase {phase}: セル数");
                assert_eq!(
                    separators + bodies,
                    EFFORT_STEPS * 2,
                    "step {step} phase {phase}: 表示幅"
                );
                assert!(
                    !gauge.contains('\u{2591}'),
                    "step {step} phase {phase}: 点字プレースホルダーがある"
                );

                // 装飾を残したまま行の続きへ抜けると、後ろの文字まで着色される。
                assert!(
                    gauge.ends_with(&format!("{ESCAPE}0m")),
                    "step {step} phase {phase}: 末尾でリセットしていない"
                );
            }
        }
    }

    /// サブエージェントの effort は数値のトークン予算のこともある。段階に
    /// 落とし込めないので、その場合はゲージを出さず数値のまま見せる。
    #[test]
    fn subagent_rows_show_the_gauge_only_for_named_levels() {
        let named = serde_json::json!({
            "id": "a", "model": "claude-opus-5", "effort": "high",
            "tokenCount": 45231, "contextWindowSize": 200000
        });
        let content = build_subagent_content(&named, None, Some(120), 0).expect("行が出ない");
        assert_eq!(content.matches(POWERLINE_RIGHT).count(), 8, "通常の楔の数");
        assert_eq!(content.matches(POWERLINE_RIGHT_THIN).count(), 1, "黒セル間の細い楔");
        assert_eq!(
            content.matches(POWERLINE_RIGHT).count() + content.matches(POWERLINE_RIGHT_THIN).count(),
            9,
            "モデル・Highの5セル・コンテキストの境界"
        );

        let budget = serde_json::json!({
            "id": "b", "model": "claude-opus-5", "effort": 12000,
            "tokenCount": 8000, "contextWindowSize": 200000
        });
        let content = build_subagent_content(&budget, None, Some(120), 0).expect("行が出ない");
        assert_eq!(
            content.matches(POWERLINE_RIGHT).count(),
            3,
            "数値予算に段数ゲージが出ている"
        );
        assert!(content.contains("12k"), "数値予算が出ていない");
    }

    /// 既知の5段階だけがゲージを持ち、未知の値や `--` では出さないこと。
    #[test]
    fn effort_gauge_only_applies_to_known_levels() {
        for (label, expected) in [
            ("Low", Some(1)),
            ("Medium", Some(2)),
            ("High", Some(3)),
            ("Xhigh", Some(4)),
            ("XHIGH", Some(4)),
            ("Max", Some(5)),
            ("--", None),
            ("Minimal", None),
            ("Turbo", None),
        ] {
            assert_eq!(get_effort_step(label), expected, "{label}");
        }

        let unknown = create_effort_text("Turbo");
        assert!(!unknown.contains(POWERLINE_RIGHT), "未知の値にゲージが出ている");
        assert_eq!(unknown, " Turbo ");
    }

    /// リセット時刻が過ぎていても表示を消さないこと。5時間の窓は5時間ごとに
    /// 切り替わるため、上限に達して利用率の更新が止まるとすぐ過去になる。
    /// そこで消えてしまうと、一番知りたいときに何も分からなくなる。
    #[test]
    fn reset_time_survives_once_it_has_passed() {
        let now = Local::now().with_timezone(&Utc);

        // 秒単位の経過で分が繰り下がるため、桁ではなく形だけを確かめる。
        let future = format_reset(Some(now + chrono::Duration::minutes(205)));
        assert!(future.contains('h') && future.contains('m'), "未来のカウントダウン: {future:?}");
        assert!(!future.ends_with("(--)"), "未来なのに古い値の印がある: {future:?}");

        let just_passed = format_reset(Some(now - chrono::Duration::hours(2)));
        assert!(!just_passed.is_empty(), "過ぎた直後に表示が消えている");
        assert!(just_passed.ends_with("(--)"), "古い値の印がない: {just_passed:?}");

        let long_passed = format_reset(Some(now - chrono::Duration::hours(30)));
        assert!(long_passed.ends_with("(--)"), "{long_passed:?}");
        assert!(long_passed.contains('/'), "1日以上前は日付で示す: {long_passed:?}");

        assert_eq!(format_reset(None), "", "値がなければ何も出さない");
    }

    /// バーは専用のトラックの上に描くので、グラデーションの全域でトラックと
    /// 十分なコントラストが必要になる。
    #[test]
    fn usage_bar_is_legible_on_its_track() {
        for percentage in 0..=100 {
            let ratio = contrast_ratio(
                get_usage_gradient(f64::from(percentage)),
                BAR_TRACK_BACKGROUND,
            );
            assert!(ratio >= 4.0, "{percentage}%: コントラスト比 {ratio:.2}");
        }

        let empty = contrast_ratio(BAR_EMPTY_FOREGROUND, BAR_TRACK_BACKGROUND);
        assert!(empty >= 1.8, "未使用部分のコントラスト比 {empty:.2}");
    }

    // ------------------------------------------------------------- --subagent

    const SUBAGENT_NOW_MS: i64 = 1_700_000_000_000;

    /// 全フィールドが揃ったタスク。`startTime` は `SUBAGENT_NOW_MS` から
    /// 134秒（2分14秒）前、`tokenSamples` は差分1つが最大値になるよう
    /// 2件だけにしてスパークラインを1文字に固定している。ラベルは30文字の
    /// ASCIIにして、削減段階ごとの幅を手計算で追えるようにしている。
    fn cascade_task(now_ms: i64) -> Value {
        serde_json::json!({
            "model": "claude-sonnet-5",
            "effort": "xhigh",
            "tokenCount": 63000.0,
            "contextWindowSize": 1_000_000.0,
            "startTime": (now_ms - 134_000) as f64,
            "tokenSamples": [0.0, 100.0],
            "name": "Bot",
            "type": "local_agent",
            "label": "x".repeat(30),
        })
    }

    /// 予算に十分余裕があれば何も削られないこと。
    #[test]
    fn subagent_content_shows_everything_when_it_fits() {
        let task = cascade_task(SUBAGENT_NOW_MS);
        let content = build_subagent_content(&task, None, Some(96), SUBAGENT_NOW_MS)
            .expect("全フィールドがあるので出る");

        assert!(content.contains("Sonnet 5"), "モデル: {content}");
        assert!(content.contains("Xhigh"), "Effort: {content}");
        assert!(
            content.contains(&background(EFFORT_EMPTY_BACKGROUND)),
            "余裕があればゲージも出る: {content}"
        );
        assert!(content.contains("63k/1M 6%"), "コンテキスト: {content}");
        assert!(content.contains("2m14s"), "経過時間: {content}");
        assert!(content.contains('\u{2588}'), "スパークライン: {content}");
        assert!(content.contains("Bot"), "エージェント名: {content}");
        assert_eq!(content.matches('x').count(), 30, "ラベルが切り詰められていない: {content}");
        assert!(!content.contains("..."), "ラベルが切り詰められていない: {content}");
    }

    /// 削減の第1段階: ゲージだけを落として段階名の文字に戻す。
    /// 段階という情報は残したまま一度に12桁空くので、ラベルより先に譲る。
    #[test]
    fn subagent_content_drops_the_gauge_before_the_label() {
        let task = cascade_task(SUBAGENT_NOW_MS);
        let content = build_subagent_content(&task, None, Some(95), SUBAGENT_NOW_MS).unwrap();

        // ゲージの空セルの黒地は、ゲージが描かれたときにしか現れない。
        assert!(
            !content.contains(&background(EFFORT_EMPTY_BACKGROUND)),
            "ゲージが消える: {content}"
        );
        assert!(content.contains("Xhigh"), "段階名は残る: {content}");
        assert!(!content.contains("..."), "ラベルはまだ切り詰めない: {content}");
        assert_eq!(content.matches('x').count(), 30, "ラベルは丸ごと残る: {content}");
        assert!(content.contains("2m14s") && content.contains('\u{2588}') && content.contains("Bot"));
    }

    /// 第2段階: ゲージを落としても収まらないので、ラベルを `...` 付きで切り詰める。
    #[test]
    fn subagent_content_truncates_label_before_dropping_anything_else() {
        let task = cascade_task(SUBAGENT_NOW_MS);
        let content = build_subagent_content(&task, None, Some(83), SUBAGENT_NOW_MS).unwrap();

        assert!(content.contains("..."), "{content}");
        assert_eq!(content.matches('x').count(), 26, "切り詰め後に残る文字数: {content}");
        assert!(content.contains("Sonnet 5") && content.contains("Xhigh") && content.contains("63k/1M 6%"));
        assert!(content.contains("2m14s") && content.contains('\u{2588}') && content.contains("Bot"));
    }

    /// 第3段階: 切り詰めても10文字未満しか残らないので、ラベルごと消える。
    #[test]
    fn subagent_content_drops_label_before_sparkline() {
        let task = cascade_task(SUBAGENT_NOW_MS);
        let content = build_subagent_content(&task, None, Some(63), SUBAGENT_NOW_MS).unwrap();

        assert!(!content.contains("..."), "{content}");
        assert_eq!(content.matches('x').count(), 0, "ラベルが完全に消える: {content}");
        assert!(content.contains('\u{2588}'), "スパークラインはまだ残る: {content}");
        assert!(content.contains("Bot") && content.contains("2m14s") && content.contains("Xhigh"));
    }

    /// 第4段階: ラベルを消しても収まらないので、次はスパークラインを落とす。
    #[test]
    fn subagent_content_drops_sparkline_before_agent() {
        let task = cascade_task(SUBAGENT_NOW_MS);
        let content = build_subagent_content(&task, None, Some(50), SUBAGENT_NOW_MS).unwrap();

        assert!(!content.contains('\u{2588}'), "スパークラインが消える: {content}");
        assert!(content.contains("Bot"), "エージェントはまだ残る: {content}");
        assert!(content.contains("2m14s") && content.contains("Xhigh"));
    }

    /// 第5段階: 次はエージェントセグメントを落とす。
    #[test]
    fn subagent_content_drops_agent_before_elapsed() {
        let task = cascade_task(SUBAGENT_NOW_MS);
        let content = build_subagent_content(&task, None, Some(48), SUBAGENT_NOW_MS).unwrap();

        assert!(!content.contains("Bot"), "エージェントが消える: {content}");
        assert!(content.contains("2m14s"), "経過時間はまだ残る: {content}");
        assert!(content.contains("Xhigh"));
    }

    /// 第6段階: 次は経過時間を落とす。結果として進捗セグメントごと消える。
    #[test]
    fn subagent_content_drops_elapsed_before_effort() {
        let task = cascade_task(SUBAGENT_NOW_MS);
        let content = build_subagent_content(&task, None, Some(42), SUBAGENT_NOW_MS).unwrap();

        assert!(!content.contains("2m14s"), "経過時間が消える: {content}");
        assert!(content.contains("Xhigh"), "Effortはまだ残る: {content}");
        assert!(content.contains("Sonnet 5") && content.contains("63k/1M 6%"));
    }

    /// 第7段階: 最後にEffortを落とす。
    #[test]
    fn subagent_content_drops_effort_last() {
        let task = cascade_task(SUBAGENT_NOW_MS);
        let content = build_subagent_content(&task, None, Some(29), SUBAGENT_NOW_MS).unwrap();

        assert!(!content.contains("Xhigh"), "Effortが消える: {content}");
        assert!(content.contains("Sonnet 5") && content.contains("63k/1M 6%"), "{content}");
    }

    /// モデルとコンテキストはどれだけ狭くても残る。
    #[test]
    fn subagent_content_always_keeps_model_and_context() {
        let task = cascade_task(SUBAGENT_NOW_MS);
        let content = build_subagent_content(&task, None, Some(10), SUBAGENT_NOW_MS).unwrap();

        assert!(
            content.contains("Sonnet 5") && content.contains("63k/1M 6%"),
            "極端に狭くてもモデルとコンテキストは残る: {content}"
        );
    }

    #[test]
    fn context_segment_shows_percentage_with_window_size() {
        let segment = build_context_segment(63000.0, Some(1_000_000.0));
        assert_eq!(segment.text, " 63k/1M 6% ");
    }

    /// 上限が無ければ使用率は出しようがないので、トークン数だけになる。
    #[test]
    fn context_segment_omits_percentage_without_window_size() {
        let segment = build_context_segment(63000.0, None);
        assert_eq!(segment.text, " 63k ");
    }

    #[test]
    fn agent_label_prefers_name_then_falls_back_to_type() {
        let cases = [
            (serde_json::json!({"name": "Explorer", "type": "local_agent"}), Some("Explorer")),
            (serde_json::json!({"type": "local_agent"}), None),
            (serde_json::json!({"type": "local_bash"}), Some("bash")),
            (serde_json::json!({"type": "local_workflow"}), Some("workflow")),
            (serde_json::json!({"type": "remote_agent"}), Some("remote")),
            (serde_json::json!({"type": "in_process_teammate"}), Some("teammate")),
            (serde_json::json!({"type": "unknown_type"}), None),
            (serde_json::json!({}), None),
        ];
        for (task, expected) in cases {
            assert_eq!(get_agent_label(&task).as_deref(), expected, "{task:?}");
        }
    }

    #[test]
    fn sparkline_handles_short_sample_lists() {
        assert_eq!(build_sparkline(&[]), None, "0件");
        assert_eq!(build_sparkline(&[42.0]), None, "1件");
    }

    /// 差分が全て0（または減少方向のみ）なら、最低段が並ぶだけで壊れない。
    #[test]
    fn sparkline_uses_lowest_level_when_flat_or_decreasing() {
        let flat = build_sparkline(&[100.0, 100.0, 100.0]).expect("2件以上なので出るはず");
        assert_eq!(flat, "\u{2581}\u{2581}");

        let decreasing = build_sparkline(&[100.0, 50.0, 10.0]).expect("2件以上なので出るはず");
        assert_eq!(decreasing, "\u{2581}\u{2581}", "減少方向のみなら最低段が並ぶ");
    }

    #[test]
    fn format_elapsed_normalizes_seconds_and_millis_the_same_way() {
        let millis = format_elapsed(Some((SUBAGENT_NOW_MS - 130_000) as f64), SUBAGENT_NOW_MS);
        assert_eq!(millis.as_deref(), Some("2m10s"));

        // 同じ瞬間を秒表記で渡しても同じ結果になること。
        let seconds = format_elapsed(Some(((SUBAGENT_NOW_MS - 130_000) / 1000) as f64), SUBAGENT_NOW_MS);
        assert_eq!(seconds.as_deref(), Some("2m10s"));
    }

    /// `startTime` が未来・欠落・不正なら、経過部分は静かに省かれる。
    #[test]
    fn format_elapsed_omits_future_missing_and_invalid_start_time() {
        assert_eq!(format_elapsed(Some((SUBAGENT_NOW_MS + 5_000) as f64), SUBAGENT_NOW_MS), None, "未来");
        assert_eq!(format_elapsed(None, SUBAGENT_NOW_MS), None, "欠落");
        assert_eq!(format_elapsed(Some(f64::NAN), SUBAGENT_NOW_MS), None, "不正");
    }

    /// `tokenSamples` が0件・1件・全差分0でも `build_subagent_content` が
    /// パニックせず、それぞれ妥当な形で出ること。
    #[test]
    fn subagent_content_survives_short_and_flat_token_samples() {
        let empty_samples =
            serde_json::json!({"model": "claude-sonnet-5", "tokenCount": 1000.0, "tokenSamples": []});
        assert!(build_subagent_content(&empty_samples, None, Some(120), SUBAGENT_NOW_MS).is_some());

        let one_sample =
            serde_json::json!({"model": "claude-sonnet-5", "tokenCount": 1000.0, "tokenSamples": [5.0]});
        assert!(build_subagent_content(&one_sample, None, Some(120), SUBAGENT_NOW_MS).is_some());

        let flat_samples = serde_json::json!({
            "model": "claude-sonnet-5",
            "tokenCount": 1000.0,
            "tokenSamples": [5.0, 5.0, 5.0],
        });
        let content = build_subagent_content(&flat_samples, None, Some(120), SUBAGENT_NOW_MS).unwrap();
        assert!(content.contains('\u{2581}'), "差分が全て0なら最低段のスパークラインになる: {content}");
    }

    /// `startTime` が秒・ミリ秒・未来・欠落のどれでも `build_subagent_content`
    /// がパニックしないこと。
    #[test]
    fn subagent_content_survives_second_and_millisecond_start_times() {
        let millis_task = serde_json::json!({
            "model": "claude-sonnet-5",
            "tokenCount": 1000.0,
            "startTime": (SUBAGENT_NOW_MS - 47_000) as f64,
        });
        let millis_content =
            build_subagent_content(&millis_task, None, Some(120), SUBAGENT_NOW_MS).unwrap();
        assert!(millis_content.contains("47s"), "{millis_content}");

        let seconds_task = serde_json::json!({
            "model": "claude-sonnet-5",
            "tokenCount": 1000.0,
            "startTime": ((SUBAGENT_NOW_MS - 47_000) / 1000) as f64,
        });
        let seconds_content =
            build_subagent_content(&seconds_task, None, Some(120), SUBAGENT_NOW_MS).unwrap();
        assert!(seconds_content.contains("47s"), "{seconds_content}");

        let future_task = serde_json::json!({
            "model": "claude-sonnet-5",
            "tokenCount": 1000.0,
            "startTime": (SUBAGENT_NOW_MS + 10_000) as f64,
        });
        let future_content =
            build_subagent_content(&future_task, None, Some(120), SUBAGENT_NOW_MS).unwrap();
        assert!(!future_content.contains('s'), "未来のstartTimeは経過を含まない: {future_content}");

        let missing_task = serde_json::json!({"model": "claude-sonnet-5", "tokenCount": 1000.0});
        let missing_content =
            build_subagent_content(&missing_task, None, Some(120), SUBAGENT_NOW_MS).unwrap();
        assert!(!missing_content.contains('s'), "startTime欠落時は経過を含まない: {missing_content}");
    }

    /// `tasks` 相当の単体（空オブジェクト、`model` 等が全欠落）では、
    /// 従来どおり行そのものを出さない。
    #[test]
    fn subagent_content_omits_line_when_everything_is_missing() {
        assert_eq!(build_subagent_content(&serde_json::json!({}), None, Some(80), SUBAGENT_NOW_MS), None);
        assert_eq!(
            build_subagent_content(
                &serde_json::json!({"type": "local_agent"}),
                None,
                None,
                SUBAGENT_NOW_MS
            ),
            None,
            "nameの無いlocal_agentは名乗れないので何も出さない"
        );
    }

    /// `tokenCount` が取れなくても、他に出すものがあれば行は出す。コンテキスト
    /// セグメントは ` -- ` になる（Effort と進捗の間を必ず埋めるため）。
    #[test]
    fn subagent_content_shows_dash_context_when_token_count_is_missing() {
        let task = serde_json::json!({
            "model": "claude-sonnet-5",
            "effort": "high",
            "name": "Bot",
            "label": "waiting",
        });
        let content = build_subagent_content(&task, None, Some(120), SUBAGENT_NOW_MS)
            .expect("tokenCountが無くても他のフィールドがあれば行を出す");
        assert!(content.contains(" -- "), "{content}");
    }

    /// フル装備のタスクから `tokenCount` だけを抜いたケース。他は全部揃って
    /// いるので、コンテキストだけが ` -- ` になり、行自体は変わらず出る。
    #[test]
    fn subagent_content_shows_dash_context_when_only_token_count_is_missing() {
        let mut task = cascade_task(SUBAGENT_NOW_MS);
        task.as_object_mut().expect("object").remove("tokenCount");

        let content = build_subagent_content(&task, None, Some(120), SUBAGENT_NOW_MS).unwrap();
        assert!(content.contains(" -- "), "{content}");
        assert!(content.contains("Sonnet 5") && content.contains("Xhigh") && content.contains("Bot"));
    }

    // --------------------------------------------------------- 表示幅（全角）

    #[test]
    fn display_width_treats_combining_marks_as_zero_width() {
        assert_eq!(display_width("e\u{0301}"), 1, "eに結合アクセントを足しても1桁");
    }

    #[test]
    fn display_width_treats_cjk_as_two_columns() {
        assert_eq!(display_width("警告"), 4);
        assert_eq!(display_width("ABC"), 3);
        assert_eq!(display_width("A警B"), 4, "半角と全角の混在");
    }

    /// スパークラインは East Asian Ambiguous だが、現代の端末では1桁で
    /// 描かれるので、全角扱いにしてはならない。
    #[test]
    fn display_width_keeps_sparkline_glyphs_at_one_column() {
        let spark = "\u{2581}\u{2582}\u{2583}\u{2584}\u{2585}\u{2586}\u{2587}\u{2588}";
        assert_eq!(display_width(spark), 8);
    }

    /// 全角文字の途中で1桁だけ余っても、そこでは切らない。
    #[test]
    fn truncate_to_width_never_splits_a_wide_character() {
        assert_eq!(truncate_to_width("警告表示", 5), "警告", "5桁では2文字目までしか入らない");
        assert_eq!(truncate_to_width("警告表示", 4), "警告");
        assert_eq!(truncate_to_width("警告表示", 3), "警", "3桁では1文字しか入らない");
        assert_eq!(truncate_to_width("ABCDE", 3), "ABC", "半角なら文字数どおり");
    }

    /// 全角ラベルでも、`columns` を絞ったときに実際の表示幅が必ず予算以下に
    /// なること。ANSIエスケープを取り除いた上で `display_width` を掛けて
    /// 確かめる（Powerlineの楔もこの中で1桁として数えている）。
    #[test]
    fn subagent_content_respects_budget_with_wide_label() {
        for columns in [300, 150, 100, 90, 84, 80, 74, 70, 65, 60, 55, 50, 45, 40, 35, 30] {
            let task = serde_json::json!({
                "model": "claude-sonnet-5",
                "effort": "xhigh",
                "tokenCount": 63000.0,
                "contextWindowSize": 1_000_000.0,
                "startTime": (SUBAGENT_NOW_MS - 134_000) as f64,
                "tokenSamples": [0.0, 100.0],
                "name": "探索エージェント",
                "type": "local_agent",
                "label": "警告表示を各セクションに適用する処理を確認しています",
            });
            let Some(content) = build_subagent_content(&task, None, Some(columns), SUBAGENT_NOW_MS) else {
                continue;
            };

            let budget = columns - 4;
            let visible_width = display_width(&strip_ansi(&content));
            assert!(
                visible_width <= budget,
                "columns={columns}: 表示幅{visible_width}が予算{budget}を超えている: {content}"
            );
        }
    }

    /// 全角＋半角混在のラベルでも同様に予算内へ収まること。
    #[test]
    fn subagent_content_respects_budget_with_mixed_width_label() {
        for columns in [300, 150, 100, 90, 84, 80, 74, 70, 65, 60, 55, 50, 45, 40, 35, 30] {
            let task = serde_json::json!({
                "model": "claude-sonnet-5",
                "effort": "high",
                "tokenCount": 12000.0,
                "contextWindowSize": 200_000.0,
                "name": "Agent警告",
                "label": "src/main.rsを読んでいます (line 1234)",
            });
            let Some(content) = build_subagent_content(&task, None, Some(columns), SUBAGENT_NOW_MS) else {
                continue;
            };

            let budget = columns - 4;
            let visible_width = display_width(&strip_ansi(&content));
            assert!(
                visible_width <= budget,
                "columns={columns}: 表示幅{visible_width}が予算{budget}を超えている: {content}"
            );
        }
    }

    /// ASCIIのみのラベルでも従来どおり予算内へ収まること（回帰確認）。
    #[test]
    fn subagent_content_respects_budget_with_ascii_only_label() {
        for columns in [300, 150, 100, 90, 84, 80, 74, 70, 65, 60, 55, 50, 45, 40, 35, 30] {
            let task = cascade_task(SUBAGENT_NOW_MS);
            let Some(content) = build_subagent_content(&task, None, Some(columns), SUBAGENT_NOW_MS) else {
                continue;
            };

            let budget = columns - 4;
            let visible_width = display_width(&strip_ansi(&content));
            assert!(
                visible_width <= budget,
                "columns={columns}: 表示幅{visible_width}が予算{budget}を超えている: {content}"
            );
        }
    }
}
