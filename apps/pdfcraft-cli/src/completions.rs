//! Shell completion script generation for `pdfcraft-cli`.

use std::fmt::Write as _;
use std::io::Write as _;

#[cfg(feature = "mcp")]
use crate::options::MCP_OPTIONS;
use crate::options::{
    AUDIT_OPTIONS, CHECK_OPTIONS, COMBINE_OPTIONS, EXTRACT_OPTIONS, INFO_OPTIONS, OptionSpec, RENDER_OPTIONS, RUN_OPTIONS, SPLIT_OPTIONS,
    TEXT_OPTIONS, UI_OPTIONS, validate_options,
};
use crate::{CliError, batch};

const SHELLS: &[&str] = &["bash", "zsh", "fish", "powershell"];

const EDIT_OPTIONS: &[OptionSpec] = &[
    OptionSpec { name: "--out", takes_value: true },
    OptionSpec { name: "--password", takes_value: true },
    OptionSpec { name: "--full", takes_value: false },
    OptionSpec { name: "--rotate", takes_value: true },
    OptionSpec { name: "--delete", takes_value: true },
    OptionSpec { name: "--move", takes_value: true },
    OptionSpec { name: "--insert-blank", takes_value: true },
    OptionSpec { name: "--title", takes_value: true },
    OptionSpec { name: "--author", takes_value: true },
];

#[cfg(feature = "mcp")]
const MCP_EXTRA_OPTIONS: &[OptionSpec] =
    &[OptionSpec { name: "--automation-read-root", takes_value: true }, OptionSpec { name: "--automation-write-root", takes_value: true }];

struct CommandSpec {
    name: &'static str,
    summary: &'static str,
    options: &'static [OptionSpec],
    extra_options: &'static [OptionSpec],
}

const COMMANDS: &[CommandSpec] = &[
    CommandSpec { name: "info", summary: "Document summary as JSON", options: INFO_OPTIONS, extra_options: &[] },
    CommandSpec { name: "render", summary: "Render a page to an image", options: RENDER_OPTIONS, extra_options: &[] },
    CommandSpec { name: "text", summary: "Extract text from a PDF", options: TEXT_OPTIONS, extra_options: &[] },
    CommandSpec { name: "edit", summary: "Apply page and metadata edits", options: EDIT_OPTIONS, extra_options: &[] },
    CommandSpec { name: "combine", summary: "Combine multiple PDFs into one", options: COMBINE_OPTIONS, extra_options: &[] },
    CommandSpec { name: "extract", summary: "Extract pages into a new PDF", options: EXTRACT_OPTIONS, extra_options: &[] },
    CommandSpec { name: "split", summary: "Split a PDF into parts", options: SPLIT_OPTIONS, extra_options: &[] },
    CommandSpec { name: "check", summary: "Run robustness checks on PDFs", options: CHECK_OPTIONS, extra_options: &[] },
    CommandSpec { name: "audit", summary: "Report redaction risks", options: AUDIT_OPTIONS, extra_options: &[] },
    CommandSpec { name: "batch", summary: "Run batch steps on files or folders", options: batch::OPTIONS, extra_options: &[] },
    CommandSpec { name: "tools", summary: "List automation tools and schemas", options: &[], extra_options: &[] },
    CommandSpec { name: "run", summary: "Run an automation tool or script", options: RUN_OPTIONS, extra_options: &[] },
    #[cfg(feature = "mcp")]
    CommandSpec { name: "mcp", summary: "Run the MCP server on stdio", options: MCP_OPTIONS, extra_options: MCP_EXTRA_OPTIONS },
    CommandSpec { name: "ui", summary: "Drive a running app over its control file", options: UI_OPTIONS, extra_options: &[] },
    CommandSpec { name: "completions", summary: "Print a shell completion script", options: &[], extra_options: &[] },
    CommandSpec { name: "help", summary: "Print usage", options: &[], extra_options: &[] },
];

/// Print a completion script for the requested shell to stdout.
pub(crate) fn completions(args: &[String]) -> Result<(), CliError> {
    validate_options("completions", args, &[])?;
    let shell = match args {
        [] => return Err("completions: missing shell (expected bash, zsh, fish or powershell)".into()),
        [shell] => shell.as_str(),
        [_, extra, ..] => {
            return Err(format!("completions: unexpected argument `{extra}` (expected one shell: bash, zsh, fish or powershell)").into());
        }
    };
    let script = generate(shell)?;
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(script.as_bytes()).and_then(|()| stdout.flush()).map_err(CliError::Stdout)
}

fn generate(shell: &str) -> Result<String, CliError> {
    match shell {
        "bash" => Ok(bash()),
        "zsh" => Ok(zsh()),
        "fish" => Ok(fish()),
        "powershell" => Ok(powershell()),
        other => Err(format!("completions: unknown shell '{other}' (expected bash, zsh, fish or powershell)").into()),
    }
}

fn all_options(cmd: &CommandSpec) -> impl Iterator<Item = &OptionSpec> {
    cmd.options.iter().chain(cmd.extra_options.iter())
}

fn is_dir_option(name: &str) -> bool {
    matches!(name, "--root" | "--out-dir" | "--automation-read-root" | "--automation-write-root")
}

fn is_file_option(name: &str) -> bool {
    matches!(name, "--out" | "--json" | "--script" | "--control")
}

fn bash() -> String {
    let mut out = String::new();
    let mut top_words = Vec::with_capacity(COMMANDS.len() + 3);
    for cmd in COMMANDS {
        top_words.push(cmd.name);
    }
    top_words.extend(["--help", "-h", "--version"]);

    #[cfg(feature = "mcp")]
    let dir_flags = "--root|--out-dir|--automation-read-root|--automation-write-root";
    #[cfg(not(feature = "mcp"))]
    let dir_flags = "--root|--out-dir";

    let _ = writeln!(out, "_pdfcraft_cli() {{");
    let _ = writeln!(out, "    local cur prev cmd");
    let _ = writeln!(out, "    cur=\"${{COMP_WORDS[COMP_CWORD]}}\"");
    let _ = writeln!(out, "    prev=\"${{COMP_WORDS[COMP_CWORD-1]}}\"");
    let _ = writeln!(out, "    if [[ ${{COMP_CWORD}} -eq 1 ]]; then");
    let _ = writeln!(out, "        COMPREPLY=( $(compgen -W \"{}\" -- \"${{cur}}\") )", top_words.join(" "));
    let _ = writeln!(out, "        return 0");
    let _ = writeln!(out, "    fi");
    let _ = writeln!(out, "    cmd=\"${{COMP_WORDS[1]}}\"");
    let _ = writeln!(out, "    case \"${{prev}}\" in");
    let _ = writeln!(out, "        {dir_flags})");
    let _ = writeln!(out, "            COMPREPLY=( $(compgen -d -- \"${{cur}}\") )");
    let _ = writeln!(out, "            return 0");
    let _ = writeln!(out, "            ;;");
    let _ = writeln!(out, "        --steps)");
    let _ = writeln!(out, "            COMPREPLY=( $(compgen -W \"convert ocr optimize\" -- \"${{cur}}\") )");
    let _ = writeln!(out, "            return 0");
    let _ = writeln!(out, "            ;;");
    let _ = writeln!(out, "    esac");
    let _ = writeln!(out, "    case \"${{cmd}}\" in");
    let _ = writeln!(out, "        completions)");
    let _ = writeln!(out, "            if [[ ${{COMP_CWORD}} -eq 2 ]]; then");
    let _ = writeln!(out, "                COMPREPLY=( $(compgen -W \"{}\" -- \"${{cur}}\") )", SHELLS.join(" "));
    let _ = writeln!(out, "            fi");
    let _ = writeln!(out, "            return 0");
    let _ = writeln!(out, "            ;;");
    for cmd in COMMANDS {
        let opts: Vec<&str> = all_options(cmd).map(|o| o.name).collect();
        if opts.is_empty() {
            continue;
        }
        let _ = writeln!(out, "        {})", cmd.name);
        let _ = writeln!(out, "            if [[ \"${{cur}}\" == -* ]]; then");
        let _ = writeln!(out, "                COMPREPLY=( $(compgen -W \"{}\" -- \"${{cur}}\") )", opts.join(" "));
        let _ = writeln!(out, "                return 0");
        let _ = writeln!(out, "            fi");
        let _ = writeln!(out, "            ;;");
    }
    let _ = writeln!(out, "    esac");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out, "complete -F _pdfcraft_cli -o bashdefault -o default pdfcraft-cli");
    out
}

fn zsh_option_spec(opt: &OptionSpec) -> String {
    if !opt.takes_value {
        return format!("'{}'", opt.name);
    }
    if is_dir_option(opt.name) {
        format!("'{}:dir:_files -/'", opt.name)
    } else if is_file_option(opt.name) {
        format!("'{}:file:_files'", opt.name)
    } else {
        format!("'{}:value:'", opt.name)
    }
}

fn zsh() -> String {
    let mut out = String::new();
    let _ = writeln!(out, "#compdef pdfcraft-cli");
    let _ = writeln!(out);
    let _ = writeln!(out, "_pdfcraft-cli() {{");
    let _ = writeln!(out, "    local -a commands");
    let _ = writeln!(out, "    commands=(");
    for cmd in COMMANDS {
        let _ = writeln!(out, "        '{}:{}'", cmd.name, cmd.summary);
    }
    let _ = writeln!(out, "    )");
    let _ = writeln!(out, "    _arguments -C \\");
    let _ = writeln!(out, "        '(-h --help)'{{-h,--help}}'[Print usage]' \\");
    let _ = writeln!(out, "        '--version[Print version]' \\");
    let _ = writeln!(out, "        '1:command:->cmd' \\");
    let _ = writeln!(out, "        '*::arg:->args'");
    let _ = writeln!(out, "    case \"$state\" in");
    let _ = writeln!(out, "        cmd)");
    let _ = writeln!(out, "            _describe -t commands 'pdfcraft-cli command' commands");
    let _ = writeln!(out, "            ;;");
    let _ = writeln!(out, "        args)");
    let _ = writeln!(out, "            case \"$line[1]\" in");
    let _ = writeln!(out, "                completions)");
    let _ = writeln!(out, "                    _arguments '1:shell:({})'", SHELLS.join(" "));
    let _ = writeln!(out, "                    ;;");
    for cmd in COMMANDS {
        let opts: Vec<String> = all_options(cmd).map(zsh_option_spec).collect();
        if opts.is_empty() {
            continue;
        }
        let _ = writeln!(out, "                {})", cmd.name);
        let _ = writeln!(out, "                    _arguments \\");
        for spec in &opts {
            let _ = writeln!(out, "                        {spec} \\");
        }
        let _ = writeln!(out, "                        '*:file:_files'");
        let _ = writeln!(out, "                    ;;");
    }
    let _ = writeln!(out, "            esac");
    let _ = writeln!(out, "            ;;");
    let _ = writeln!(out, "    esac");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);
    let _ = writeln!(out, "if [ \"$funcstack[1]\" = \"_pdfcraft-cli\" ]; then");
    let _ = writeln!(out, "    _pdfcraft-cli \"$@\"");
    let _ = writeln!(out, "else");
    let _ = writeln!(out, "    compdef _pdfcraft-cli pdfcraft-cli");
    let _ = writeln!(out, "fi");
    out
}

fn fish() -> String {
    let mut out = String::new();
    let _ = writeln!(out, "complete -c pdfcraft-cli -n '__fish_use_subcommand' -s h -l help -d 'Print usage'");
    let _ = writeln!(out, "complete -c pdfcraft-cli -n '__fish_use_subcommand' -l version -d 'Print version'");
    for cmd in COMMANDS {
        let _ = writeln!(out, "complete -c pdfcraft-cli -f -n '__fish_use_subcommand' -a '{}' -d '{}'", cmd.name, cmd.summary);
    }
    let _ = writeln!(out, "complete -c pdfcraft-cli -f -n '__fish_seen_subcommand_from completions' -a '{}'", SHELLS.join(" "));
    for cmd in COMMANDS {
        for opt in all_options(cmd) {
            let long_name = opt.name.strip_prefix("--").unwrap_or(opt.name);
            if is_dir_option(opt.name) {
                let _ = writeln!(
                    out,
                    "complete -c pdfcraft-cli -n '__fish_seen_subcommand_from {}' -l '{long_name}' -x -a '(__fish_complete_directories)'",
                    cmd.name
                );
            } else if opt.takes_value {
                let _ = writeln!(out, "complete -c pdfcraft-cli -n '__fish_seen_subcommand_from {}' -l '{long_name}' -r", cmd.name);
            } else {
                let _ = writeln!(out, "complete -c pdfcraft-cli -n '__fish_seen_subcommand_from {}' -l '{long_name}'", cmd.name);
            }
        }
    }
    out
}

fn powershell() -> String {
    let mut out = String::new();
    let mut top_items: Vec<String> = COMMANDS.iter().map(|c| format!("'{}'", c.name)).collect();
    top_items.extend(["'--help'".into(), "'-h'".into(), "'--version'".into()]);
    let shell_items: Vec<String> = SHELLS.iter().map(|s| format!("'{s}'")).collect();

    let _ = writeln!(out, "Register-ArgumentCompleter -Native -CommandName @('pdfcraft-cli', 'pdfcraft-cli.exe') -ScriptBlock {{");
    let _ = writeln!(out, "    param($wordToComplete, $commandAst, $cursorPosition)");
    let _ = writeln!(out, "    $elements = @($commandAst.CommandElements | ForEach-Object {{ $_.Extent.Text }})");
    let _ = writeln!(out, "    $cmd = if ($elements.Count -ge 2 -and ($elements.Count -gt 2 -or -not $wordToComplete)) {{");
    let _ = writeln!(out, "        $elements[1]");
    let _ = writeln!(out, "    }} else {{");
    let _ = writeln!(out, "        $null");
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out, "    $items = switch ($cmd) {{");
    let _ = writeln!(out, "        $null {{ @({}) }}", top_items.join(", "));
    let _ = writeln!(out, "        'completions' {{ @({}) }}", shell_items.join(", "));
    for cmd in COMMANDS {
        let opts: Vec<String> = all_options(cmd).map(|o| format!("'{}'", o.name)).collect();
        if opts.is_empty() {
            continue;
        }
        let _ = writeln!(out, "        '{}' {{ @({}) }}", cmd.name, opts.join(", "));
    }
    let _ = writeln!(out, "        default {{ @() }}");
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out, "    $items | Where-Object {{ $_ -like \"$wordToComplete*\" }} | ForEach-Object {{");
    let _ = writeln!(out, "        [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)");
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out, "}}");
    out
}
