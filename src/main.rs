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
const POWERLINE_RIGHT: char = '\u{e0b0}';
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
const EFFORT_BACKGROUND: Color = rgb(155, 89, 182);
const CONTEXT_LOW_BACKGROUND: Color = rgb(205, 154, 11);
const CONTEXT_MEDIUM_BACKGROUND: Color = rgb(245, 196, 24);
const CONTEXT_HIGH_BACKGROUND: Color = rgb(230, 126, 34);
const CONTEXT_CRITICAL_BACKGROUND: Color = rgb(231, 76, 60);
const DIRECTORY_BACKGROUND: Color = rgb(111, 78, 176);
const RATE_LOW_BACKGROUND: Color = rgb(22, 135, 119);
const RATE_MEDIUM_BACKGROUND: Color = rgb(205, 154, 11);
const RATE_HIGH_BACKGROUND: Color = rgb(192, 57, 43);
const GIT_BACKGROUND: Color = rgb(41, 128, 185);
const WORKTREE_BACKGROUND: Color = rgb(92, 72, 165);
const DIFF_BACKGROUND: Color = rgb(39, 174, 96);
const WHITE: Color = rgb(255, 255, 255);
const DARK_TEXT: Color = rgb(30, 36, 45);

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

    let context_foreground = get_context_foreground(context.percentage_value);
    let mut segments = vec![
        Segment {
            background: MODEL_BACKGROUND,
            foreground: WHITE,
            text: format!(" Model: {} ", get_model_name(root)),
        },
        Segment {
            background: EFFORT_BACKGROUND,
            foreground: WHITE,
            text: format!(" Effort: {} ", get_main_effort(root)),
        },
        Segment {
            background: get_context_background(context.percentage_value),
            foreground: context_foreground,
            text: create_context_text(&context, context_foreground),
        },
        Segment {
            background: DIRECTORY_BACKGROUND,
            foreground: WHITE,
            text: format!(" Cwd: {} ", format_directory(&directory)),
        },
        create_rate_segment("5h", rates.five_hour),
        create_rate_segment("7d", rates.seven_day),
    ];

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
                    segments.push(Segment {
                        background: DIFF_BACKGROUND,
                        foreground: WHITE,
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

fn create_context_text(context: &Context, foreground: Color) -> String {
    match context.percentage_value {
        None => format!(" Ctx: {}/{} --% ", context.current, context.maximum),
        Some(value) => format!(
            " Ctx: {}/{} {} {}% ",
            context.current,
            context.maximum,
            build_usage_bar(value, foreground),
            context.percentage
        ),
    }
}

fn build_usage_bar(percentage: f64, segment_foreground: Color) -> String {
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

    let mut bar = String::new();
    for _ in 0..full.max(0) {
        bar.push(BLOCKS[8]);
    }
    if fraction > 0 {
        bar.push(BLOCKS[fraction as usize]);
    }
    for _ in 0..empty.max(0) {
        bar.push('\u{2591}');
    }

    // Only the bar receives this SGR foreground; restore the segment foreground
    // immediately without resetting its background or the Powerline connection.
    format!(
        "{}{}{}",
        foreground(get_usage_gradient(clamped)),
        bar,
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

fn get_context_foreground(percentage: Option<f64>) -> Color {
    match percentage {
        Some(value) if value >= 95.0 => WHITE,
        _ => DARK_TEXT,
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

fn create_rate_segment(label: &str, limit: RateLimit) -> Segment {
    let percentage = limit.percentage;
    let foreground = match percentage {
        Some(value) if value >= 80.0 => WHITE,
        _ => DARK_TEXT,
    };
    let reset = format_reset(limit.resets_at);

    match percentage {
        None => Segment {
            background: get_rate_background(None),
            foreground,
            text: format!(" {}: --%{} ", label, reset),
        },
        Some(value) => Segment {
            background: get_rate_background(Some(value)),
            foreground,
            text: format!(
                " {} {} {}%{} ",
                label,
                build_usage_bar(value, foreground),
                format_percentage(value),
                reset
            ),
        },
    }
}

fn get_rate_background(percentage: Option<f64>) -> Color {
    match percentage {
        Some(value) if value >= 80.0 => RATE_HIGH_BACKGROUND,
        Some(value) if value >= 50.0 => RATE_MEDIUM_BACKGROUND,
        _ => RATE_LOW_BACKGROUND,
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

    let session_effort = get_effort(root);
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
