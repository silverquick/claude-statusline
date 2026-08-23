using System.Diagnostics;
using System.Globalization;
using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;

internal static class Program
{
    private const string Escape = "\x1b[";
    private const string PowerlineRight = "";
    private const string EmptyTree = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

    private static readonly Color ModelBackground = new(185, 49, 49);
    private static readonly Color EffortBackground = new(155, 89, 182);
    private static readonly Color ContextLowBackground = new(205, 154, 11);
    private static readonly Color ContextMediumBackground = new(245, 196, 24);
    private static readonly Color ContextHighBackground = new(230, 126, 34);
    private static readonly Color ContextCriticalBackground = new(231, 76, 60);
    private static readonly Color DirectoryBackground = new(111, 78, 176);
    private static readonly Color RateLowBackground = new(22, 135, 119);
    private static readonly Color RateMediumBackground = new(205, 154, 11);
    private static readonly Color RateHighBackground = new(192, 57, 43);
    private static readonly Color GitBackground = new(41, 128, 185);
    private static readonly Color WorktreeBackground = new(92, 72, 165);
    private static readonly Color DiffBackground = new(39, 174, 96);
    private static readonly Color White = new(255, 255, 255);
    private static readonly Color DarkText = new(30, 36, 45);

    private static int Main(string[] args)
    {
        Console.InputEncoding = new UTF8Encoding(false);
        Console.OutputEncoding = new UTF8Encoding(false);

        try
        {
            var input = Console.In.ReadToEnd();
            using var document = ParseObject(input);
            if (args.Length > 0 && string.Equals(args[0], "--subagent", StringComparison.Ordinal))
            {
                WriteSubagentStatus(document.RootElement);
            }
            else
            {
                Console.Out.Write(BuildStatusLine(document.RootElement));
            }
        }
        catch
        {
            // A status line must never interfere with Claude Code's UI.
        }

        return 0;
    }

    private static JsonDocument ParseObject(string input)
    {
        try
        {
            var parsed = JsonDocument.Parse(input);
            if (parsed.RootElement.ValueKind == JsonValueKind.Object)
            {
                return parsed;
            }

            parsed.Dispose();
        }
        catch
        {
            // Fall through to the empty object below.
        }

        return JsonDocument.Parse("{}");
    }

    private static string BuildStatusLine(JsonElement root)
    {
        var directory = GetWorkingDirectory(root);
        var context = GetContext(root);
        var rates = GetRateLimits(root);
        var repository = FindRepository(directory, GetWorkspaceWorktree(root));

        var contextForeground = GetContextForeground(context.PercentageValue);
        var segments = new List<Segment>
        {
            new(ModelBackground, White, $" Model: {GetModelName(root)} "),
            new(EffortBackground, White, $" Effort: {GetMainEffort(root)} "),
            new(
                GetContextBackground(context.PercentageValue),
                contextForeground,
                CreateContextText(context, contextForeground)),
            new(DirectoryBackground, White, $" Cwd: {FormatDirectory(directory)} "),
            CreateRateSegment("5h", rates.FiveHour),
            CreateRateSegment("7d", rates.SevenDay)
        };

        if (repository is { Branch: not null } repositoryValue)
        {
            segments.Add(new Segment(GitBackground, White, $" ⎇ {CleanText(repositoryValue.Branch)} "));
            if (!string.IsNullOrWhiteSpace(repositoryValue.Worktree))
            {
                segments.Add(new Segment(WorktreeBackground, White, $" WT: {CleanText(repositoryValue.Worktree)} "));
            }

            var diff = GetDiffStat(repositoryValue, directory);
            if (diff is not null && (diff.Value.Added > 0 || diff.Value.Deleted > 0))
            {
                segments.Add(new Segment(
                    DiffBackground,
                    White,
                    $" (+{diff.Value.Added.ToString(CultureInfo.InvariantCulture)},-{diff.Value.Deleted.ToString(CultureInfo.InvariantCulture)}) "));
            }
        }

        return RenderPowerline(segments);
    }

    private static string GetModelName(JsonElement root)
    {
        if (TryGetObject(root, "model", out var model))
        {
            var name = GetString(model, "display_name");
            if (string.IsNullOrWhiteSpace(name))
            {
                name = GetString(model, "id");
            }

            return string.IsNullOrWhiteSpace(name) ? "?" : CleanText(name);
        }

        return "?";
    }

    private static string GetMainEffort(JsonElement root)
    {
        string? effort = null;
        if (TryGetObject(root, "effort", out var effortObject))
        {
            effort = GetString(effortObject, "level");
        }

        effort ??= GetString(root, "effort");
        effort ??= GetString(root, "effortLevel");
        effort ??= GetString(root, "reasoningEffort");
        if (string.IsNullOrWhiteSpace(effort))
        {
            return "--";
        }

        var cleaned = CleanText(effort).Trim();
        return cleaned.Length == 0 ? "--" : Capitalize(cleaned);
    }

    private static string CreateContextText(Context context, Color foreground)
    {
        if (context.PercentageValue is null)
        {
            return $" Ctx: {context.Current}/{context.Maximum} --% ";
        }

        return $" Ctx: {context.Current}/{context.Maximum} {BuildUsageBar(context.PercentageValue.Value, foreground)} {context.Percentage}% ";
    }

    private static string BuildUsageBar(double percentage, Color segmentForeground)
    {
        const int width = 10;
        const string blocks = " ▏▎▍▌▋▊▉█";
        var clamped = Math.Clamp(percentage, 0d, 100d);
        var filled = clamped * width / 100d;
        var full = Math.Min(width, (int)Math.Floor(filled));
        var fraction = full == width ? 0 : (int)Math.Floor((filled - full) * 8d);
        var empty = width - full - (fraction > 0 ? 1 : 0);
        var bar = new StringBuilder(width)
            .Append(blocks[8], full)
            .Append(fraction > 0 ? blocks[fraction] : '\0', fraction > 0 ? 1 : 0)
            .Append('░', empty)
            .ToString();

        // Only the bar receives this SGR foreground; restore the segment foreground
        // immediately without resetting its background or the Powerline connection.
        return Foreground(GetUsageGradient(clamped)) + bar + Foreground(segmentForeground);
    }

    private static Color GetUsageGradient(double percentage)
    {
        var clamped = Math.Clamp(percentage, 0d, 100d);
        if (clamped < 50d)
        {
            return new Color((int)(clamped * 5.1d), 200, 80);
        }

        return new Color(255, Math.Max((int)(200d - (clamped - 50d) * 4d), 0), 60);
    }

    private static Context GetContext(JsonElement root)
    {
        if (!TryGetObject(root, "context_window", out var contextWindow))
        {
            return new Context("--", "--", "--", null);
        }

        // Claude Code's combined total is the current input context, while current_usage
        // is its component breakdown. Output tokens are deliberately never included.
        double? currentValue = TryGetNumber(contextWindow, "total_input_tokens", out var totalInput)
            ? totalInput
            : GetCurrentUsageTotal(contextWindow);
        var maximumValue = TryGetNumber(contextWindow, "context_window_size", out var maximum) ? maximum : (double?)null;
        double? rawPercentage = TryGetNumber(contextWindow, "used_percentage", out var reportedPercentage)
            ? reportedPercentage
            : currentValue is not null && maximumValue is > 0
                ? currentValue.Value / maximumValue.Value * 100d
                : null;

        // The same AwayFromZero display value drives text, colour, and its bar.
        double? percentageValue = rawPercentage is null ? null : NormalizePercentage(rawPercentage.Value);
        return new Context(
            currentValue is null ? "--" : FormatCompactNumber(currentValue.Value),
            maximumValue is null ? "--" : FormatCompactNumber(maximumValue.Value),
            percentageValue is null ? "--" : FormatPercentage(percentageValue.Value),
            percentageValue);
    }

    private static double? GetCurrentUsageTotal(JsonElement contextWindow)
    {
        if (!TryGetObject(contextWindow, "current_usage", out var usage))
        {
            return null;
        }

        var found = false;
        var total = 0d;
        total += GetNumber(usage, ref found, "input_tokens", "input");
        total += GetNumber(usage, ref found, "cache_creation_input_tokens", "cache_creation");
        total += GetNumber(usage, ref found, "cache_read_input_tokens", "cache_read");
        return found ? total : null;
    }

    private static Color GetContextBackground(double? percentage) => percentage switch
    {
        >= 95d => ContextCriticalBackground,
        >= 85d => ContextHighBackground,
        >= 70d => ContextMediumBackground,
        _ => ContextLowBackground
    };

    private static Color GetContextForeground(double? percentage) => percentage is >= 95d ? White : DarkText;

    private static double GetNumber(JsonElement element, ref bool found, params string[] propertyNames)
    {
        foreach (var propertyName in propertyNames)
        {
            if (TryGetNumber(element, propertyName, out var value))
            {
                found = true;
                return value;
            }
        }

        return 0;
    }

    private static string FormatCompactNumber(double value)
    {
        if (double.IsNaN(value) || double.IsInfinity(value))
        {
            return "?";
        }

        var absolute = Math.Abs(value);
        if (absolute >= 1_000_000)
        {
            return (value / 1_000_000d).ToString("0.0", CultureInfo.InvariantCulture) + "M";
        }

        if (absolute >= 1_000)
        {
            return (value / 1_000d).ToString("0.#", CultureInfo.InvariantCulture) + "k";
        }

        return value.ToString("0", CultureInfo.InvariantCulture);
    }

    private static double NormalizePercentage(double value) =>
        Math.Round(value, MidpointRounding.AwayFromZero);

    private static string FormatPercentage(double value) =>
        NormalizePercentage(value).ToString("0", CultureInfo.InvariantCulture);

    private static string GetWorkingDirectory(JsonElement root)
    {
        var directory = TryGetObject(root, "workspace", out var workspace)
            ? GetString(workspace, "current_dir")
            : null;
        if (string.IsNullOrWhiteSpace(directory))
        {
            directory = GetString(root, "cwd");
        }

        directory = string.IsNullOrWhiteSpace(directory) ? Environment.CurrentDirectory : directory;
        try
        {
            return Path.GetFullPath(directory);
        }
        catch
        {
            return Environment.CurrentDirectory;
        }
    }

    private static string FormatDirectory(string directory)
    {
        var display = CleanText(directory);
        try
        {
            var home = Path.GetFullPath(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile))
                .TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
            var full = Path.GetFullPath(directory).TrimEnd(Path.DirectorySeparatorChar, Path.AltDirectorySeparatorChar);
            if (string.Equals(full, home, StringComparison.OrdinalIgnoreCase))
            {
                return "~";
            }

            var homePrefix = home + Path.DirectorySeparatorChar;
            if (full.StartsWith(homePrefix, StringComparison.OrdinalIgnoreCase))
            {
                return "~" + full[home.Length..];
            }
        }
        catch
        {
            // Keep the already-clean path below.
        }

        return display;
    }

    private static string? GetWorkspaceWorktree(JsonElement root)
    {
        if (TryGetObject(root, "workspace", out var workspace))
        {
            var value = GetString(workspace, "git_worktree");
            return string.IsNullOrWhiteSpace(value) ? null : CleanText(value.Trim());
        }

        return null;
    }

    private static RateLimits GetRateLimits(JsonElement root)
    {
        var hasRateLimits = TryGetObject(root, "rate_limits", out var rateLimits);
        JsonElement fiveHour = default;
        JsonElement sevenDay = default;
        var hasFiveHour = hasRateLimits && TryGetObject(rateLimits, "five_hour", out fiveHour);
        var hasSevenDay = hasRateLimits && TryGetObject(rateLimits, "seven_day", out sevenDay);
        var cached = !hasFiveHour || !hasSevenDay ? ReadCachedRateLimits() : null;

        return new RateLimits(
            hasFiveHour ? ParseRateLimit(fiveHour) : cached?.FiveHour ?? RateLimit.Empty,
            hasSevenDay ? ParseRateLimit(sevenDay) : cached?.SevenDay ?? RateLimit.Empty);
    }

    private static RateLimit ParseRateLimit(JsonElement limit)
    {
        var percentage = TryGetNumber(limit, "used_percentage", out var value) ? NormalizePercentage(value) : (double?)null;
        var reset = limit.TryGetProperty("resets_at", out var resetElement) ? ParseResetTime(resetElement) : null;
        return new RateLimit(percentage, reset);
    }

    private static RateLimits? ReadCachedRateLimits()
    {
        try
        {
            var path = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), ".claude.json");
            if (!File.Exists(path))
            {
                return null;
            }

            using var document = JsonDocument.Parse(File.ReadAllText(path, Encoding.UTF8));
            if (!TryGetObject(document.RootElement, "cachedUsageUtilization", out var cached) ||
                !TryGetObject(cached, "utilization", out var utilization))
            {
                return null;
            }

            return new RateLimits(
                TryGetObject(utilization, "five_hour", out var fiveHour) ? ParseCachedRateLimit(fiveHour) : RateLimit.Empty,
                TryGetObject(utilization, "seven_day", out var sevenDay) ? ParseCachedRateLimit(sevenDay) : RateLimit.Empty);
        }
        catch
        {
            return null;
        }
    }

    private static RateLimit ParseCachedRateLimit(JsonElement limit)
    {
        var percentage = TryGetNumber(limit, "utilization", out var value) ? NormalizePercentage(value) : (double?)null;
        var reset = limit.TryGetProperty("resets_at", out var resetElement) ? ParseResetTime(resetElement) : null;
        return new RateLimit(percentage, reset);
    }

    private static DateTimeOffset? ParseResetTime(JsonElement value)
    {
        try
        {
            if (value.ValueKind == JsonValueKind.Number && value.TryGetDouble(out var seconds) &&
                !double.IsNaN(seconds) && !double.IsInfinity(seconds))
            {
                return DateTimeOffset.FromUnixTimeSeconds(checked((long)Math.Floor(seconds)));
            }

            if (value.ValueKind == JsonValueKind.String)
            {
                var text = value.GetString();
                if (double.TryParse(text, NumberStyles.Float, CultureInfo.InvariantCulture, out var epoch))
                {
                    return DateTimeOffset.FromUnixTimeSeconds(checked((long)Math.Floor(epoch)));
                }

                if (DateTimeOffset.TryParse(
                    text,
                    CultureInfo.InvariantCulture,
                    DateTimeStyles.AssumeUniversal | DateTimeStyles.AdjustToUniversal,
                    out var parsed))
                {
                    return parsed;
                }
            }
        }
        catch
        {
            // Invalid or out-of-range timestamps omit only the reset suffix.
        }

        return null;
    }

    private static Segment CreateRateSegment(string label, RateLimit limit)
    {
        var percentage = limit.Percentage;
        var foreground = percentage is >= 80d ? White : DarkText;
        var reset = FormatReset(limit.ResetsAt);
        if (percentage is null)
        {
            return new Segment(GetRateBackground(null), foreground, $" {label}: --%{reset} ");
        }

        return new Segment(
            GetRateBackground(percentage),
            foreground,
            $" {label} {BuildUsageBar(percentage.Value, foreground)} {FormatPercentage(percentage.Value)}%{reset} ");
    }

    private static Color GetRateBackground(double? percentage) => percentage switch
    {
        >= 80d => RateHighBackground,
        >= 50d => RateMediumBackground,
        _ => RateLowBackground
    };

    private static string FormatReset(DateTimeOffset? reset)
    {
        if (reset is null)
        {
            return string.Empty;
        }

        var local = reset.Value.ToLocalTime();
        var remaining = local - DateTimeOffset.Now;
        if (remaining <= TimeSpan.Zero)
        {
            return string.Empty;
        }

        if (remaining < TimeSpan.FromDays(1))
        {
            return $" {local:HH:mm}({(int)remaining.TotalHours}h{remaining.Minutes}m)";
        }

        return $" {local:M/d}({(int)remaining.TotalDays}d{remaining.Hours}h)";
    }

    private static Repository? FindRepository(string startDirectory, string? configuredWorktree)
    {
        try
        {
            var directory = new DirectoryInfo(startDirectory);
            while (directory is not null)
            {
                var dotGit = Path.Combine(directory.FullName, ".git");
                if (Directory.Exists(dotGit))
                {
                    return new Repository(ReadBranch(Path.Combine(dotGit, "HEAD")), null, dotGit, !HasHeadCommit(dotGit));
                }

                if (File.Exists(dotGit))
                {
                    var gitDirectory = ResolveGitDirectory(dotGit);
                    if (gitDirectory is null)
                    {
                        return new Repository(null, null, null, false);
                    }

                    var derivedWorktree = GetLinkedWorktreeName(gitDirectory);
                    var worktree = !string.IsNullOrWhiteSpace(configuredWorktree) ? configuredWorktree : derivedWorktree;
                    return new Repository(ReadBranch(Path.Combine(gitDirectory, "HEAD")), worktree, gitDirectory, !HasHeadCommit(gitDirectory));
                }

                directory = directory.Parent;
            }
        }
        catch
        {
            // A file-system failure simply hides the Git segments.
        }

        return null;
    }

    private static string? GetLinkedWorktreeName(string gitDirectory)
    {
        try
        {
            var info = new DirectoryInfo(gitDirectory);
            return string.Equals(info.Parent?.Name, "worktrees", StringComparison.OrdinalIgnoreCase)
                ? info.Name
                : null;
        }
        catch
        {
            return null;
        }
    }

    private static string? ResolveGitDirectory(string dotGitFile)
    {
        try
        {
            var text = File.ReadAllText(dotGitFile, Encoding.UTF8).Trim();
            const string prefix = "gitdir:";
            if (!text.StartsWith(prefix, StringComparison.OrdinalIgnoreCase))
            {
                return null;
            }

            var path = text[prefix.Length..].Trim();
            if (path.Length == 0)
            {
                return null;
            }

            if (!Path.IsPathRooted(path))
            {
                path = Path.GetFullPath(Path.Combine(Path.GetDirectoryName(dotGitFile)!, path));
            }

            return Directory.Exists(path) ? path : null;
        }
        catch
        {
            return null;
        }
    }

    private static string? ReadBranch(string headPath)
    {
        try
        {
            var head = File.ReadAllText(headPath, Encoding.UTF8).Trim();
            const string prefix = "ref: refs/heads/";
            if (head.StartsWith(prefix, StringComparison.Ordinal))
            {
                var branch = head[prefix.Length..].Trim();
                return branch.Length == 0 ? null : branch;
            }

            return head.Length == 0 ? null : head[..Math.Min(8, head.Length)];
        }
        catch
        {
            return null;
        }
    }

    private static bool HasHeadCommit(string gitDirectory)
    {
        try
        {
            var head = File.ReadAllText(Path.Combine(gitDirectory, "HEAD"), Encoding.UTF8).Trim();
            const string prefix = "ref: ";
            if (!head.StartsWith(prefix, StringComparison.Ordinal))
            {
                return head.Length > 0;
            }

            var reference = head[prefix.Length..].Trim();
            if (reference.Length == 0)
            {
                return false;
            }

            // A linked worktree stores HEAD locally but refs and packed-refs in its
            // common Git directory, named by the local commondir file.
            var commonDirectory = GetCommonGitDirectory(gitDirectory);
            var looseReference = Path.Combine(commonDirectory, reference.Replace('/', Path.DirectorySeparatorChar));
            if (File.Exists(looseReference) && !string.IsNullOrWhiteSpace(File.ReadAllText(looseReference, Encoding.UTF8)))
            {
                return true;
            }

            var packedReferences = Path.Combine(commonDirectory, "packed-refs");
            return File.Exists(packedReferences) && File.ReadLines(packedReferences, Encoding.UTF8).Any(line =>
                !line.StartsWith("#", StringComparison.Ordinal) &&
                !line.StartsWith("^", StringComparison.Ordinal) &&
                line.EndsWith(" " + reference, StringComparison.Ordinal));
        }
        catch
        {
            return true;
        }
    }

    private static string GetCommonGitDirectory(string gitDirectory)
    {
        try
        {
            var commonDirectoryFile = Path.Combine(gitDirectory, "commondir");
            if (!File.Exists(commonDirectoryFile))
            {
                return gitDirectory;
            }

            var commonDirectory = File.ReadAllText(commonDirectoryFile, Encoding.UTF8).Trim();
            if (commonDirectory.Length == 0)
            {
                return gitDirectory;
            }

            if (!Path.IsPathRooted(commonDirectory))
            {
                commonDirectory = Path.GetFullPath(Path.Combine(gitDirectory, commonDirectory));
            }

            return Directory.Exists(commonDirectory) ? commonDirectory : gitDirectory;
        }
        catch
        {
            return gitDirectory;
        }
    }

    private static DiffStat? GetDiffStat(Repository repository, string directory)
    {
        try
        {
            using var process = new Process();
            process.StartInfo = new ProcessStartInfo
            {
                FileName = "git",
                UseShellExecute = false,
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                CreateNoWindow = true,
                WorkingDirectory = directory
            };
            process.StartInfo.ArgumentList.Add("-c");
            process.StartInfo.ArgumentList.Add("core.fsmonitor=false");
            process.StartInfo.ArgumentList.Add("--no-optional-locks");
            process.StartInfo.ArgumentList.Add("-C");
            process.StartInfo.ArgumentList.Add(directory);
            process.StartInfo.ArgumentList.Add("diff");
            process.StartInfo.ArgumentList.Add("--numstat");
            if (repository.IsUnborn)
            {
                process.StartInfo.ArgumentList.Add("--cached");
                process.StartInfo.ArgumentList.Add(EmptyTree);
            }
            else
            {
                process.StartInfo.ArgumentList.Add("HEAD");
            }

            process.StartInfo.ArgumentList.Add("--");
            if (!process.Start())
            {
                return null;
            }

            var stdoutTask = process.StandardOutput.ReadToEndAsync();
            var stderrTask = process.StandardError.ReadToEndAsync();
            if (!process.WaitForExit(3_000))
            {
                process.Kill(entireProcessTree: true);
                return null;
            }

            var stdout = stdoutTask.GetAwaiter().GetResult();
            _ = stderrTask.GetAwaiter().GetResult();
            if (process.ExitCode != 0)
            {
                return null;
            }

            long added = 0;
            long deleted = 0;
            foreach (var line in stdout.Split('\n', StringSplitOptions.RemoveEmptyEntries))
            {
                var fields = line.Split('\t');
                if (fields.Length < 2 || fields[0] == "-" || fields[1] == "-")
                {
                    continue;
                }

                if (long.TryParse(fields[0], NumberStyles.None, CultureInfo.InvariantCulture, out var lineAdded))
                {
                    added += lineAdded;
                }

                if (long.TryParse(fields[1], NumberStyles.None, CultureInfo.InvariantCulture, out var lineDeleted))
                {
                    deleted += lineDeleted;
                }
            }

            return new DiffStat(added, deleted);
        }
        catch
        {
            return null;
        }
    }

    private static string RenderPowerline(IReadOnlyList<Segment> segments)
    {
        var output = new StringBuilder();
        for (var index = 0; index < segments.Count; index++)
        {
            var segment = segments[index];
            if (index == 0)
            {
                output.Append(Background(segment.Background));
            }
            else
            {
                var previous = segments[index - 1];
                output.Append(Foreground(previous.Background));
                output.Append(Background(segment.Background));
                output.Append(PowerlineRight);
            }

            output.Append(Foreground(segment.Foreground));
            output.Append(segment.Text);
        }

        var finalBackground = segments[^1].Background;
        output.Append(Foreground(finalBackground));
        output.Append(Escape).Append("49m");
        output.Append(PowerlineRight);
        output.Append(Escape).Append("0m");
        return output.ToString();
    }

    private static string Foreground(Color color) =>
        $"{Escape}38;2;{color.Red};{color.Green};{color.Blue}m";

    private static string Background(Color color) =>
        $"{Escape}48;2;{color.Red};{color.Green};{color.Blue}m";

    private static void WriteSubagentStatus(JsonElement root)
    {
        if (!TryGetArray(root, "tasks", out var tasks))
        {
            return;
        }

        var sessionEffort = GetEffort(root);
        var columns = TryGetNumber(root, "columns", out var columnValue) && columnValue > 0
            ? (int)Math.Min(columnValue, int.MaxValue)
            : (int?)null;

        foreach (var task in tasks.EnumerateArray())
        {
            if (task.ValueKind != JsonValueKind.Object)
            {
                continue;
            }

            var id = GetString(task, "id");
            if (string.IsNullOrEmpty(id))
            {
                continue;
            }

            var content = BuildSubagentContent(task, sessionEffort, columns);
            if (content is not null)
            {
                Console.Out.WriteLine(JsonSerializer.Serialize(new SubagentLine(id, content)));
            }
        }
    }

    private static string? BuildSubagentContent(JsonElement task, string? sessionEffort, int? columns)
    {
        var colored = new List<string>();
        var plain = new List<string>();

        var model = GetString(task, "model");
        if (!string.IsNullOrEmpty(model))
        {
            var label = PrettifyModel(CleanText(model));
            colored.Add(Ansi("36", label));
            plain.Add(label);
        }

        var effort = GetTaskEffort(task, sessionEffort);
        if (effort is not null)
        {
            colored.Add(Ansi("35", effort));
            plain.Add(effort);
        }

        if (TryGetNumber(task, "tokenCount", out var tokenCount))
        {
            string label;
            if (TryGetNumber(task, "contextWindowSize", out var contextSize) && contextSize > 0)
            {
                var displayedPercentage = Math.Round(tokenCount / contextSize * 100d, MidpointRounding.AwayFromZero);
                label = $"{FormatRoundedK(tokenCount)}/{FormatRoundedContext(contextSize)} {displayedPercentage.ToString(CultureInfo.InvariantCulture)}%";
                colored.Add(Ansi(PercentageColor(displayedPercentage), label));
            }
            else
            {
                label = FormatRoundedK(tokenCount);
                colored.Add(Ansi("2", label));
            }

            plain.Add(label);
        }

        if (colored.Count == 0)
        {
            return null;
        }

        var headColored = string.Join(" ", colored);
        var headPlain = string.Join(" ", plain);
        var description = GetString(task, "description") ?? GetString(task, "name");
        description = description is null ? null : CleanText(description);
        if (string.IsNullOrEmpty(description))
        {
            return headColored;
        }

        var available = (columns ?? 60) - headPlain.Length - 3;
        if (available < 10)
        {
            return headColored;
        }

        if (description.Length > available)
        {
            description = description[..Math.Max(0, available - 3)] + "...";
        }

        return headColored + Ansi("2", " · " + description);
    }

    private static string? GetTaskEffort(JsonElement task, string? sessionEffort)
    {
        if (task.TryGetProperty("effort", out var effort))
        {
            if (effort.ValueKind == JsonValueKind.String)
            {
                var value = effort.GetString();
                return string.IsNullOrEmpty(value) ? null : Capitalize(CleanText(value));
            }

            if (TryGetNumber(effort, out var budget))
            {
                return FormatRoundedK(budget);
            }
        }

        return sessionEffort is null ? null : Capitalize(CleanText(sessionEffort));
    }

    private static string? GetEffort(JsonElement root)
    {
        if (root.TryGetProperty("effort", out var effort))
        {
            if (effort.ValueKind == JsonValueKind.String)
            {
                var value = effort.GetString();
                return value is null ? null : CleanText(value);
            }

            if (effort.ValueKind == JsonValueKind.Object)
            {
                var level = GetString(effort, "level");
                return level is null ? null : CleanText(level);
            }
        }

        return null;
    }

    private static string PrettifyModel(string model)
    {
        return model switch
        {
            "claude-fable-5" => "Fable 5",
            "claude-opus-5" => "Opus 5",
            "claude-sonnet-5" => "Sonnet 5",
            _ when model.StartsWith("claude-haiku-4-5", StringComparison.Ordinal) => "Haiku 4.5",
            _ => PrettifyModelFallback(model)
        };
    }

    private static string PrettifyModelFallback(string model)
    {
        var text = model.StartsWith("claude-", StringComparison.Ordinal) ? model[7..] : model;
        text = Regex.Replace(text, "-\\d{6,8}$", string.Empty);
        var parts = text.Split('-', StringSplitOptions.RemoveEmptyEntries);
        if (parts.Length == 0)
        {
            return model;
        }

        parts[0] = Capitalize(parts[0]);
        return string.Join(" ", parts);
    }

    private static string FormatRoundedK(double value) =>
        Math.Round(value / 1_000d, MidpointRounding.AwayFromZero).ToString(CultureInfo.InvariantCulture) + "k";

    private static string FormatRoundedContext(double value) => value >= 1_000_000d
        ? Math.Round(value / 1_000_000d, MidpointRounding.AwayFromZero).ToString(CultureInfo.InvariantCulture) + "M"
        : Math.Round(value / 1_000d, MidpointRounding.AwayFromZero).ToString(CultureInfo.InvariantCulture) + "k";

    private static string PercentageColor(double displayedPercentage) => displayedPercentage >= 90d ? "31" : displayedPercentage >= 70d ? "33" : "36";

    private static string Capitalize(string value) => value.Length == 0
        ? value
        : char.ToUpperInvariant(value[0]) + value[1..];

    private static string Ansi(string code, string text) => $"{Escape}{code}m{text}{Escape}0m";

    private static string CleanText(string value)
    {
        var output = new StringBuilder(value.Length);
        foreach (var character in value)
        {
            output.Append(char.IsControl(character) ? ' ' : character);
        }

        return output.ToString();
    }

    private static bool TryGetObject(JsonElement element, string propertyName, out JsonElement value)
    {
        if (element.ValueKind == JsonValueKind.Object && element.TryGetProperty(propertyName, out value) && value.ValueKind == JsonValueKind.Object)
        {
            return true;
        }

        value = default;
        return false;
    }

    private static bool TryGetArray(JsonElement element, string propertyName, out JsonElement value)
    {
        if (element.ValueKind == JsonValueKind.Object && element.TryGetProperty(propertyName, out value) && value.ValueKind == JsonValueKind.Array)
        {
            return true;
        }

        value = default;
        return false;
    }

    private static string? GetString(JsonElement element, string propertyName)
    {
        return element.ValueKind == JsonValueKind.Object && element.TryGetProperty(propertyName, out var value) && value.ValueKind == JsonValueKind.String
            ? value.GetString()
            : null;
    }

    private static bool TryGetNumber(JsonElement element, string propertyName, out double number)
    {
        if (element.ValueKind == JsonValueKind.Object && element.TryGetProperty(propertyName, out var value))
        {
            return TryGetNumber(value, out number);
        }

        number = default;
        return false;
    }

    private static bool TryGetNumber(JsonElement element, out double number)
    {
        if (element.ValueKind == JsonValueKind.Number && element.TryGetDouble(out number))
        {
            return true;
        }

        number = default;
        return false;
    }

    private readonly record struct Color(int Red, int Green, int Blue);
    private readonly record struct Segment(Color Background, Color Foreground, string Text);
    private readonly record struct Context(string Current, string Maximum, string Percentage, double? PercentageValue);
    private readonly record struct RateLimit(double? Percentage, DateTimeOffset? ResetsAt)
    {
        public static RateLimit Empty => new(null, null);
    }

    private readonly record struct RateLimits(RateLimit FiveHour, RateLimit SevenDay);
    private readonly record struct Repository(string? Branch, string? Worktree, string? GitDirectory, bool IsUnborn);
    private readonly record struct DiffStat(long Added, long Deleted);
    private sealed record SubagentLine(string id, string content);
}
