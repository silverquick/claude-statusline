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
// 細い区切りは、両側の背景色が文字どおり同一で実線の楔が原理的に描けない
// 箇所にだけ使う。Effort ゲージの未点灯どうしがこれにあたる。
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

// Effort は連続値ではなく5段階なので、割合を示すバーではなく段の数を示す
// ゲージで描く。ここでは2つの別々の見分けが同時に要る。
//
//   点灯と未点灯の区別 … 1マス幅の図と地の判別なので、輝度比が効く。
//                        色差だけ大きくても暗い色どうしでは分離しない。
//   段どうしの区別     … 数えられる必要があるので、色差を確保する。
//
// そのため点灯色は「確実に明るい」帯の中だけでランプを組み、未点灯は
// はっきり暗くしている。塗りは1本の紫系ランプで、位置が上がるほど明るい。
// Effort が高いほど明るい段まで届き、ゲージ全体が強く見える。
const EFFORT_STEPS: usize = 5;
// ゲージは現在の段階まで1段ずつ点いていき、そこで止まる。段は3つの状態を取る。
//
//   点灯   すでに点いた段。彩度の高い紫
//   待機   現在の段階の範囲内だが、まだ点いていない段。点灯と同じ明度で低彩度
//   未点灯 現在の段階より上の段。暗い地に `░`
//
// 待機色を「同じ明度・低彩度」にしているのは、CIE Lab では輝度が L* だけで
// 決まるためで、彩度を落としても未点灯との輝度比は変わらない。おかげで
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
// 端末の地の上へ描くときのゲージの表示幅。段5つと、段間4つ＋末尾1つの区切り。
const EFFORT_GAUGE_WIDTH: usize = EFFORT_STEPS * 2;
const EFFORT_EMPTY_BACKGROUND: Color = rgb(35, 29, 41);
// 未点灯セルには使用率バーと同じ `░` を置く。色だけに頼らない手がかりを
// 足すことで、点灯との違いと段数の両方が読み取れるようにする。
const EFFORT_EMPTY_FOREGROUND: Color = rgb(94, 86, 105);
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
            text: create_effort_text(&get_main_effort(root)),
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

/// Effort の表示。既知の5段階なら見出しのあとに段数ゲージを添える。
/// 未知の値や `--` のときは、置く位置が決められないのでゲージを出さない。
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

/// 5段を斜めの境界でつないだゲージ。各段は「区切り＋1マスの地」でできており、
/// 区切りの前景に前の段の色、背景に次の段の色を置くことで台形が連続して見える。
/// 最後にセグメント本来の色へ戻すので、外側の Powerline 接続には影響しない。
/// 5段ゲージを組み立てる。`surround` はゲージの外側の背景色で、Powerline
/// セグメントの中に描くときはその背景色を渡す。サブエージェント行のように
/// 端末の地の上へ直接描くときは `None` を渡す。外側の色が分からないと
/// ゲージ先頭の楔は描けないので、その場合は最初の段から始める。
fn build_effort_gauge(step: usize, phase: usize, surround: Option<Color>) -> String {
    // 1コマ目は何も点いていない状態から始め、1段ずつ点けていき、現在の段階に
    // 達したらしばらくそのまま留まる。段階より上の段には決して届かない。
    let cycle = step + 1 + EFFORT_HOLD_FRAMES;
    let lit = (phase % cycle).min(step);

    let mut gauge = String::new();
    let mut previous = surround;

    for index in 0..EFFORT_STEPS {
        let current = if index < lit {
            EFFORT_LIT_BACKGROUNDS[index]
        } else if index < step {
            EFFORT_PENDING_BACKGROUNDS[index]
        } else {
            EFFORT_EMPTY_BACKGROUND
        };

        match previous {
            // 未点灯どうしは背景が同一で、実線の楔は原理的に描けない。
            // 5段あることが分かるよう、細い区切りで段の切れ目だけを示す。
            Some(prev) if prev == current => {
                gauge.push_str(&foreground(EFFORT_EMPTY_DIVIDER));
                gauge.push(POWERLINE_RIGHT_THIN);
            }
            Some(prev) => {
                gauge.push_str(&foreground(prev));
                gauge.push_str(&background(current));
                gauge.push(POWERLINE_RIGHT);
            }
            None => gauge.push_str(&background(current)),
        }

        if index < step {
            gauge.push(' ');
        } else {
            gauge.push_str(&foreground(EFFORT_EMPTY_FOREGROUND));
            gauge.push('\u{2591}');
        }

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
    if remaining <= chrono::Duration::zero() {
        return String::new();
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

fn ansi(code: &str, text: &str) -> String {
    format!("{}{}m{}{}0m", ESCAPE, code, text, ESCAPE)
}

// ------------------------------------------------------------------ subagents

fn write_subagent_status(root: &Value) {
    let Some(tasks) = root.get("tasks").and_then(Value::as_array) else {
        return;
    };

    let session_effort = get_effort(root).or_else(|| get_transcript_effort(root));
    let columns = get_number(root, "columns").filter(|value| *value > 0.0).map(|value| value as i64);

    for task in tasks {
        if !task.is_object() {
            continue;
        }

        let Some(id) = get_string(task, "id").filter(|value| !value.is_empty()) else {
            continue;
        };

        if let Some(content) = build_subagent_content(task, session_effort.as_deref(), columns) {
            println!(
                "{{\"id\":{},\"content\":{}}}",
                Value::String(id.to_string()),
                Value::String(content)
            );
        }
    }
}

fn build_subagent_content(task: &Value, session_effort: Option<&str>, columns: Option<i64>) -> Option<String> {
    let mut colored: Vec<String> = Vec::new();
    let mut plain: Vec<String> = Vec::new();

    if let Some(model) = get_string(task, "model").filter(|value| !value.is_empty()) {
        let label = prettify_model(&clean_text(model));
        colored.push(ansi("36", &label));
        plain.push(label);
    }

    if let Some(effort) = get_task_effort(task, session_effort) {
        colored.push(ansi("35", &effort));
        // 段階が既知のときだけゲージを添える。サブエージェントの effort は
        // 数値のトークン予算でもありうるので、その場合は数値のまま出す。
        if let Some(step) = get_effort_step(&effort) {
            colored.push(build_effort_gauge(step, current_phase(), None));
            plain.push(" ".repeat(EFFORT_GAUGE_WIDTH));
        }
        plain.push(effort);
    }

    if let Some(token_count) = get_number(task, "tokenCount") {
        let label = match get_number(task, "contextWindowSize").filter(|value| *value > 0.0) {
            Some(context_size) => {
                let displayed = round_half_away_from_zero(token_count / context_size * 100.0);
                let label = format!(
                    "{}/{} {}%",
                    format_rounded_k(token_count),
                    format_rounded_context(context_size),
                    format_integer(displayed)
                );
                colored.push(ansi(percentage_color(displayed), &label));
                label
            }
            None => {
                let label = format_rounded_k(token_count);
                colored.push(ansi("2", &label));
                label
            }
        };

        plain.push(label);
    }

    if colored.is_empty() {
        return None;
    }

    let head_colored = colored.join(" ");
    let head_plain = plain.join(" ");
    let description = get_string(task, "description")
        .or_else(|| get_string(task, "name"))
        .map(clean_text)
        .filter(|value| !value.is_empty());

    let Some(mut description) = description else {
        return Some(head_colored);
    };

    let available = columns.unwrap_or(60) - head_plain.chars().count() as i64 - 3;
    if available < 10 {
        return Some(head_colored);
    }

    if description.chars().count() as i64 > available {
        let keep = (available - 3).max(0) as usize;
        description = description.chars().take(keep).collect::<String>() + "...";
    }

    Some(head_colored + &ansi("2", &format!(" \u{b7} {}", description)))
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

fn percentage_color(displayed_percentage: f64) -> &'static str {
    if displayed_percentage >= 90.0 {
        "31"
    } else if displayed_percentage >= 70.0 {
        "33"
    } else {
        "36"
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

    /// どの段階・どのコマでも5段が描かれ、段階の範囲内の段と範囲外の段の数が
    /// 一致すること。区切りは「5段ぶん＋ゲージを閉じるぶん」で6個になる。
    #[test]
    fn effort_gauge_always_shows_five_steps() {
        for step in 1..=EFFORT_STEPS {
            for phase in 0..step + 1 + EFFORT_HOLD_FRAMES {
                let gauge = build_effort_gauge(step, phase, Some(EFFORT_BACKGROUND));

                let separators = gauge.matches(POWERLINE_RIGHT).count()
                    + gauge.matches(POWERLINE_RIGHT_THIN).count();
                assert_eq!(separators, EFFORT_STEPS + 1, "step {step} phase {phase}: 区切りの数");

                assert_eq!(
                    gauge.matches(' ').count(),
                    step,
                    "step {step} phase {phase}: 段階の範囲内の段の数"
                );
                assert_eq!(
                    gauge.matches('\u{2591}').count(),
                    EFFORT_STEPS - step,
                    "step {step} phase {phase}: 段階の範囲外の段の数"
                );
            }
        }
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

        let divider = contrast_ratio(EFFORT_EMPTY_DIVIDER, EFFORT_EMPTY_BACKGROUND);
        assert!(divider >= 2.0, "細区切りのコントラスト比 {divider:.2}");

        let glyph = contrast_ratio(EFFORT_EMPTY_FOREGROUND, EFFORT_EMPTY_BACKGROUND);
        assert!(glyph >= 2.0, "未点灯の記号のコントラスト比 {glyph:.2}");
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

    /// サブエージェント行は Powerline セグメントの中ではなく端末の地の上に
    /// 描く。外側の背景色が分からないので先頭の楔は描かず、幅は段5つ＋区切り
    /// 5つの10桁になる。説明文の切り詰め計算がこの幅に依存している。
    #[test]
    fn effort_gauge_on_terminal_ground_has_a_known_width() {
        for step in 1..=EFFORT_STEPS {
            for phase in 0..step + 1 + EFFORT_HOLD_FRAMES {
                let gauge = build_effort_gauge(step, phase, None);

                let separators = gauge.matches(POWERLINE_RIGHT).count()
                    + gauge.matches(POWERLINE_RIGHT_THIN).count();
                let bodies = gauge.matches(' ').count() + gauge.matches('\u{2591}').count();
                assert_eq!(bodies, EFFORT_STEPS, "step {step} phase {phase}: 段の数");
                assert_eq!(
                    separators + bodies,
                    EFFORT_GAUGE_WIDTH,
                    "step {step} phase {phase}: 表示幅"
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
        let content = build_subagent_content(&named, None, Some(120)).expect("行が出ない");
        assert!(content.contains(POWERLINE_RIGHT), "既知の段階にゲージが出ていない");

        let budget = serde_json::json!({
            "id": "b", "model": "claude-opus-5", "effort": 12000,
            "tokenCount": 8000, "contextWindowSize": 200000
        });
        let content = build_subagent_content(&budget, None, Some(120)).expect("行が出ない");
        assert!(!content.contains(POWERLINE_RIGHT), "数値予算にゲージが出ている");
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
}
