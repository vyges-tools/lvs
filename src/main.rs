//! vyges-lvs CLI.
//!
//!   vyges-lvs run   JOB [-o OUT] [--json] [--fail-on-mismatch]   compare -> report
//!   vyges-lvs check JOB                                          validate the job
//!   vyges-lvs demo  [-o OUT] [--json]                            built-in pair
//!
//! Common flags: -h/--help, -V/--version, -q/--quiet, -v/--verbose.
//! Exit codes: 0 ok · 1 runtime/parse error · 2 usage/validation · 3 LVS mismatch
//! (only with --fail-on-mismatch).

use std::process::exit;

use vyges_lvs::compare::LvsResult;
use vyges_lvs::engine;
use vyges_lvs::job::LvsJob;

const USAGE: &str = "\
vyges-lvs — layout-vs-schematic netlist comparison with clear divergence diagnostics

usage:
  vyges-lvs run     JOB [-o OUT] [--json] [--fail-on-mismatch]
  vyges-lvs extract GDS --rules RULES [--top CELL] [-o out.spice]
  vyges-lvs check   JOB
  vyges-lvs demo         [-o OUT] [--json]

A JOB is a small declarative `.lvs` file: the layout side as a SPICE netlist
(`layout:`) OR a GDS to extract natively (`layout_gds:` + `rules:`), plus the
`schematic:` and an optional `top:`. The compare is name-independent (graph
colour-refinement); a mismatch reports the unmatched device/net classes.
`extract` runs native device extraction (GDS/OASIS -> SPICE) on its own.

flags:
  -o FILE              write the report to FILE (default: stdout)
  --json               machine-readable JSON instead of the text report
  --fail-on-mismatch   exit 3 if the netlists are not equivalent (CI gate)
  -q, --quiet          suppress non-essential output
  -v, --verbose        extra detail on stderr
  -h, --help           show this help
  -V, --version        show version
  --bug-report         file a bug (central: vyges/community)
  --feature-request    request a feature (central)
  --sponsor            sponsor Vyges (github.com/sponsors/vyges-ip)
  --star               star this tool on GitHub ⭐
";

const BUG_URL: &str =
    "https://github.com/vyges/community/issues/new?template=bug_report_template.yaml";
const FEATURE_URL: &str = "https://github.com/vyges/community/issues/new?labels=enhancement";
const SPONSOR_URL: &str = "https://github.com/sponsors/vyges-ip";
const STAR_URL: &str = "https://github.com/vyges-tools/lvs";

fn link(label: &str, url: &str) {
    use std::io::IsTerminal;
    println!("{label}:\n  {url}");
    if std::io::stdout().is_terminal() {
        let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
        let _ = std::process::Command::new(opener).arg(url).status();
    }
}

#[derive(Default)]
struct Cli {
    positionals: Vec<String>,
    out: Option<String>,
    json: bool,
    quiet: bool,
    verbose: bool,
    fail_on_mismatch: bool,
    help: bool,
    version: bool,
    bug_report: bool,
    feature_request: bool,
    sponsor: bool,
    star: bool,
    rules: Option<String>,
    top: Option<String>,
}

fn parse_cli(args: &[String]) -> Cli {
    let mut c = Cli::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                c.out = args.get(i + 1).cloned();
                i += 1;
            }
            "--rules" => {
                c.rules = args.get(i + 1).cloned();
                i += 1;
            }
            "--top" => {
                c.top = args.get(i + 1).cloned();
                i += 1;
            }
            "--json" => c.json = true,
            "--fail-on-mismatch" => c.fail_on_mismatch = true,
            "-q" | "--quiet" => c.quiet = true,
            "-v" | "--verbose" => c.verbose = true,
            "-h" | "--help" => c.help = true,
            "-V" | "--version" => c.version = true,
            "--bug-report" => c.bug_report = true,
            "--feature-request" => c.feature_request = true,
            "--sponsor" => c.sponsor = true,
            "--star" => c.star = true,
            other => c.positionals.push(other.to_string()),
        }
        i += 1;
    }
    c
}

fn write_out(text: &str, cli: &Cli) {
    match &cli.out {
        Some(path) => match std::fs::write(path, text) {
            Ok(_) => {
                if !cli.quiet {
                    println!("wrote {path}");
                }
            }
            Err(e) => {
                eprintln!("error: {path}: {e}");
                exit(1);
            }
        },
        None => print!("{text}"),
    }
}

fn emit(r: &LvsResult, cli: &Cli) -> ! {
    let text = if cli.json { engine::report_json(r) } else { engine::render_report(r) };
    write_out(&text, cli);
    if cli.fail_on_mismatch && !r.matched {
        if !cli.quiet {
            eprintln!("LVS MISMATCH");
        }
        exit(3);
    }
    exit(0);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = parse_cli(&args);

    if cli.bug_report {
        return link("Report a bug (central — vyges/community)", BUG_URL);
    }
    if cli.feature_request {
        return link("Request a feature (central — vyges/community)", FEATURE_URL);
    }
    if cli.sponsor {
        return link("Sponsor Vyges", SPONSOR_URL);
    }
    if cli.star {
        return link("Star vyges-lvs on GitHub ⭐", STAR_URL);
    }
    if cli.version {
        println!("vyges-lvs {} ({})", vyges_lvs::VERSION, env!("VYGES_GIT_SHA"));
        println!("{}", vyges_lvs::COPYRIGHT);
        return;
    }
    let cmd = cli.positionals.first().cloned().unwrap_or_default();
    if cli.help || cmd.is_empty() {
        print!("{USAGE}");
        exit(if cmd.is_empty() && !cli.help { 2 } else { 0 });
    }

    match cmd.as_str() {
        "demo" => emit(&engine::demo(), &cli),
        "extract" => {
            let Some(gds) = cli.positionals.get(1) else {
                eprintln!("usage: vyges-lvs extract GDS --rules RULES [--top CELL] [-o out.spice]");
                exit(2);
            };
            let Some(rules_path) = &cli.rules else {
                eprintln!("usage: vyges-lvs extract GDS --rules RULES");
                exit(2);
            };
            let rules = match vyges_lvs::extract::Rules::load(rules_path) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("error: {e}");
                    exit(2);
                }
            };
            match vyges_lvs::extract::extract_file(gds, cli.top.as_deref(), &rules) {
                Ok(nl) => {
                    let spice = vyges_lvs::extract::to_spice(&nl);
                    match &cli.out {
                        Some(p) => std::fs::write(p, &spice).unwrap_or_else(|e| {
                            eprintln!("error: {p}: {e}");
                            exit(1);
                        }),
                        None => print!("{spice}"),
                    }
                    if cli.verbose {
                        eprintln!("extracted {} device(s), {} port(s)", nl.devices.len(), nl.ports.len());
                    }
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    exit(1);
                }
            }
        }
        "check" => {
            let Some(path) = cli.positionals.get(1) else {
                eprintln!("usage: vyges-lvs check JOB");
                exit(2);
            };
            match LvsJob::load(path) {
                Ok(j) => println!(
                    "OK  layout={} schematic={} top={}",
                    j.layout_gds.as_deref().or(j.layout.as_deref()).unwrap_or("?"),
                    j.schematic,
                    j.top.as_deref().unwrap_or("(top-level)")
                ),
                Err(e) => {
                    eprintln!("error: {e}");
                    exit(2);
                }
            }
        }
        "run" => {
            let Some(path) = cli.positionals.get(1) else {
                eprintln!("usage: vyges-lvs run JOB [-o OUT]");
                exit(2);
            };
            let job = match LvsJob::load(path) {
                Ok(j) => j,
                Err(e) => {
                    eprintln!("error: {e}");
                    exit(2);
                }
            };
            if cli.verbose {
                let src = job.layout_gds.as_deref().or(job.layout.as_deref()).unwrap_or("?");
                eprintln!("comparing {} vs {}", src, job.schematic);
            }
            match engine::run_job(&job) {
                Ok(r) => emit(&r, &cli),
                Err(e) => {
                    eprintln!("error: {e}");
                    exit(1);
                }
            }
        }
        other => {
            eprintln!("vyges-lvs: unknown command {other:?}\n");
            print!("{USAGE}");
            exit(2);
        }
    }
}
