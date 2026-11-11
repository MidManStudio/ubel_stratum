//! From Ubel source to a native executable (M1a of `docs/LLVM_BACKEND_STATUS.md`).
//!
//! The pipeline is: [`crate::compile`] (front end and emitter) gives textual LLVM IR; the IR
//! and the C runtime are written to a private temporary directory; one `clang` invocation
//! assembles the IR, compiles the runtime and links the executable. There is no dependency
//! on libLLVM, only on a `clang` of version 15 or newer being installed.
//!
//! The runtime (`runtime/*.c`, `runtime/*.h`) is embedded in this crate with `include_str!`,
//! so a build needs nothing from the source tree. It is compiled again on every build; there
//! is no cache yet.
//!
//! Provisional: the default optimisation level, the compiler search order and the way the
//! runtime is linked can all still change.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::CompileError;

/// The C runtime, embedded. All four files are written next to the generated IR, because
/// `ubel_rt.c` and `ubel_main.c` include the two headers.
const RUNTIME_FILES: [(&str, &str); 4] = [
    ("ubel_rt.h", include_str!("../runtime/ubel_rt.h")),
    ("ubel_unicode.h", include_str!("../runtime/ubel_unicode.h")),
    ("ubel_rt.c", include_str!("../runtime/ubel_rt.c")),
    ("ubel_main.c", include_str!("../runtime/ubel_main.c")),
];

/// The oldest clang whose IR parser takes what the emitter writes.
pub const MIN_CLANG_MAJOR: u32 = 15;

/// Names tried, in order, when no compiler was asked for. The versioned names come first so
/// that a machine with several clangs gets the newest one that was tested; the plain `clang`
/// is last because its version is unknown until it is asked.
const AUTO_CANDIDATES: [&str; 5] = ["clang-18", "clang-17", "clang-16", "clang-15", "clang"];

// ---------------------------------------------------------------------------------------
// Options and results
// ---------------------------------------------------------------------------------------

/// The `-O` level handed to clang.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptLevel {
    O0,
    O1,
    #[default]
    O2,
    O3,
    Os,
}

impl OptLevel {
    /// `"0"`, `"1"`, `"2"`, `"3"` or `"s"`.
    pub fn parse(s: &str) -> Option<OptLevel> {
        match s {
            "0" => Some(OptLevel::O0),
            "1" => Some(OptLevel::O1),
            "2" => Some(OptLevel::O2),
            "3" => Some(OptLevel::O3),
            "s" => Some(OptLevel::Os),
            _ => None,
        }
    }

    pub fn flag(self) -> &'static str {
        match self {
            OptLevel::O0 => "-O0",
            OptLevel::O1 => "-O1",
            OptLevel::O2 => "-O2",
            OptLevel::O3 => "-O3",
            OptLevel::Os => "-Os",
        }
    }
}

#[derive(Debug, Clone)]
pub struct BuildOptions {
    /// Where the executable goes. Its directory must already exist.
    pub output: PathBuf,
    /// The compiler to use. `None` means: `UBEL_CC` if it is set, else search the PATH.
    pub cc: Option<PathBuf>,
    pub opt: OptLevel,
    /// Extra clang arguments (for example `-m32` or a sanitizer flag). They go after the
    /// `-O` level and before the inputs, so they can override it.
    pub extra_flags: Vec<String>,
    /// Keep the temporary directory (the IR and the runtime files) after the build.
    pub keep_temps: bool,
    /// Where the temporary directory is created. `None` means the system temporary directory.
    pub tmp_base: Option<PathBuf>,
}

impl BuildOptions {
    pub fn new(output: impl Into<PathBuf>) -> Self {
        BuildOptions {
            output: output.into(),
            cc: None,
            opt: OptLevel::default(),
            extra_flags: Vec::new(),
            keep_temps: false,
            tmp_base: None,
        }
    }
}

/// The version line of a clang, as far as it could be understood.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClangVersion {
    pub major: u32,
    /// Apple's clang numbers its releases differently from LLVM's, so no minimum is enforced.
    pub apple: bool,
    /// The line of `--version` that holds the version, for messages.
    pub line: String,
}

/// A compiler that was found and answered `--version` like a clang.
#[derive(Debug, Clone)]
pub struct CompilerInfo {
    pub path: PathBuf,
    pub version: ClangVersion,
}

#[derive(Debug, Clone)]
pub struct BuildOutput {
    pub output: PathBuf,
    pub compiler: CompilerInfo,
    /// The full clang command line, for display.
    pub command: String,
    /// What clang printed on stderr although it succeeded (usually nothing).
    pub warnings: String,
    /// The temporary directory, only when `keep_temps` was set.
    pub kept_dir: Option<PathBuf>,
}

#[derive(Debug)]
pub enum BuildError {
    /// The front end or the emitter refused the program.
    Compile(CompileError),
    /// `--cc` or `UBEL_CC` named a compiler that is not there.
    ExplicitNotFound { source: &'static str, name: String },
    /// No clang on the PATH under any of the names tried.
    NoCompiler { tried: Vec<String> },
    /// A compiler was found but cannot be used (it does not run, or is not clang).
    CompilerUnusable { path: PathBuf, reason: String },
    CompilerTooOld { path: PathBuf, version_line: String, major: u32 },
    OutputDirMissing(PathBuf),
    Io { what: String, source: io::Error },
    /// clang ran and failed. `stderr` is what it printed.
    CompilerFailed { command: String, status: String, stderr: String, kept_dir: Option<PathBuf> },
}

impl From<CompileError> for BuildError {
    fn from(e: CompileError) -> Self {
        BuildError::Compile(e)
    }
}

/// At most this many lines of the compiler's stderr go into an error message.
const STDERR_LINES_SHOWN: usize = 30;

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::Compile(e) => write!(f, "{e}"),
            BuildError::ExplicitNotFound { source, name } => {
                write!(f, "the C compiler given with {source} ('{name}') was not found")
            }
            BuildError::NoCompiler { tried } => write!(
                f,
                "no clang found (looked for {}); install clang {MIN_CLANG_MAJOR} or newer, \
                 or point UBEL_CC or --cc at one",
                tried.join(", ")
            ),
            BuildError::CompilerUnusable { path, reason } => {
                write!(f, "cannot use {} as the C compiler: {reason}", path.display())
            }
            BuildError::CompilerTooOld { path, version_line, .. } => write!(
                f,
                "{} is too old ({version_line}); clang {MIN_CLANG_MAJOR} or newer is needed",
                path.display()
            ),
            BuildError::OutputDirMissing(p) => {
                write!(f, "the output directory {} does not exist", p.display())
            }
            BuildError::Io { what, source } => write!(f, "{what}: {source}"),
            BuildError::CompilerFailed { command, status, stderr, kept_dir } => {
                write!(f, "the C compiler failed ({status})\n  command: {command}")?;
                let lines: Vec<&str> = stderr.trim_end().lines().collect();
                for line in lines.iter().take(STDERR_LINES_SHOWN) {
                    write!(f, "\n  | {line}")?;
                }
                if lines.len() > STDERR_LINES_SHOWN {
                    write!(f, "\n  | ... {} more lines", lines.len() - STDERR_LINES_SHOWN)?;
                }
                match kept_dir {
                    Some(d) => write!(f, "\n  kept: {}", d.display()),
                    None => write!(
                        f,
                        "\n  (re-run with --keep-temps to keep the generated IR and the runtime files)"
                    ),
                }
            }
        }
    }
}

impl std::error::Error for BuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BuildError::Compile(e) => Some(e),
            BuildError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------------------
// Finding a compiler
// ---------------------------------------------------------------------------------------

/// Read the version out of the text `clang --version` prints. The line looks like
/// `clang version 18.1.3 (...)`, with a vendor in front on some systems
/// (`Ubuntu clang version ...`, `Debian clang version ...`, `Apple clang version ...`).
pub fn parse_clang_version(text: &str) -> Option<ClangVersion> {
    const KEY: &str = "clang version ";
    let line = text.lines().find(|l| l.contains(KEY))?;
    let rest = &line[line.find(KEY)? + KEY.len()..];
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let major = digits.parse().ok()?;
    Some(ClangVersion { major, apple: line.contains("Apple clang"), line: line.trim().to_string() })
}

#[cfg(unix)]
fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match fs::metadata(p) {
        Ok(m) => m.is_file() && m.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable_file(p: &Path) -> bool {
    fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
}

/// A name with a directory part is checked as it is; a bare name is searched for in `path_var`.
fn which(name: &OsStr, path_var: Option<&OsStr>) -> Option<PathBuf> {
    let has_dir = Path::new(name).parent().map_or(false, |p| !p.as_os_str().is_empty());
    if has_dir {
        let p = PathBuf::from(name);
        return if is_executable_file(&p) { Some(p) } else { None };
    }
    for dir in std::env::split_paths(path_var?) {
        let plain = dir.join(name);
        if is_executable_file(&plain) {
            return Some(plain);
        }
        if cfg!(windows) {
            let mut exe = OsString::from(name);
            exe.push(".exe");
            let with_ext = dir.join(exe);
            if is_executable_file(&with_ext) {
                return Some(with_ext);
            }
        }
    }
    None
}

/// Run `path --version` and check that it is a clang that is new enough.
fn probe(path: &Path) -> Result<CompilerInfo, BuildError> {
    let unusable = |reason: String| BuildError::CompilerUnusable { path: path.to_path_buf(), reason };
    let out = Command::new(path)
        .arg("--version")
        .output()
        .map_err(|e| unusable(format!("it cannot be run: {e}")))?;
    if !out.status.success() {
        return Err(unusable(format!("`--version` failed ({})", out.status)));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let version = parse_clang_version(&text).ok_or_else(|| {
        let first: String = text.lines().next().unwrap_or("").chars().take(80).collect();
        unusable(format!("it does not look like clang (`--version` starts with: {first})"))
    })?;
    if !version.apple && version.major < MIN_CLANG_MAJOR {
        return Err(BuildError::CompilerTooOld {
            path: path.to_path_buf(),
            version_line: version.line.clone(),
            major: version.major,
        });
    }
    Ok(CompilerInfo { path: path.to_path_buf(), version })
}

fn probe_named(name: &OsStr, source: &'static str, path_var: Option<&OsStr>) -> Result<CompilerInfo, BuildError> {
    match which(name, path_var) {
        Some(p) => probe(&p),
        None => Err(BuildError::ExplicitNotFound { source, name: name.to_string_lossy().into_owned() }),
    }
}

/// The search, with its inputs passed in. `explicit` (from `--cc`) wins over `env_cc` (from
/// `UBEL_CC`), which wins over searching `path_var`. A compiler that was asked for by name is
/// never replaced by another one when it is unusable. During the automatic search a candidate
/// that cannot be used is skipped, and the first such problem is reported if nothing else works.
pub fn find_compiler_with(
    explicit: Option<&Path>,
    env_cc: Option<&OsStr>,
    path_var: Option<&OsStr>,
) -> Result<CompilerInfo, BuildError> {
    if let Some(p) = explicit {
        return probe_named(p.as_os_str(), "--cc", path_var);
    }
    if let Some(v) = env_cc {
        if !v.is_empty() {
            return probe_named(v, "UBEL_CC", path_var);
        }
    }
    let mut first_problem: Option<BuildError> = None;
    for name in AUTO_CANDIDATES {
        if let Some(found) = which(OsStr::new(name), path_var) {
            match probe(&found) {
                Ok(info) => return Ok(info),
                Err(e) => {
                    if first_problem.is_none() {
                        first_problem = Some(e);
                    }
                }
            }
        }
    }
    Err(first_problem.unwrap_or_else(|| BuildError::NoCompiler {
        tried: AUTO_CANDIDATES.iter().map(|s| s.to_string()).collect(),
    }))
}

/// Find the compiler from the process environment (`UBEL_CC`, `PATH`).
pub fn find_compiler(explicit: Option<&Path>) -> Result<CompilerInfo, BuildError> {
    let env_cc = std::env::var_os("UBEL_CC");
    let path_var = std::env::var_os("PATH");
    find_compiler_with(explicit, env_cc.as_deref(), path_var.as_deref())
}

// ---------------------------------------------------------------------------------------
// Building
// ---------------------------------------------------------------------------------------

/// A private directory that is removed again when it goes out of scope, unless `keep` is set.
struct TempDir {
    path: PathBuf,
    keep: bool,
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(unix)]
fn make_private_dir(p: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(p)
}

#[cfg(not(unix))]
fn make_private_dir(p: &Path) -> io::Result<()> {
    fs::create_dir(p)
}

impl TempDir {
    fn create(base: &Path, keep: bool) -> io::Result<TempDir> {
        let pid = std::process::id();
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        for _ in 0..100 {
            let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = base.join(format!("ubel-build-{pid}-{nanos}-{n}"));
            match make_private_dir(&path) {
                Ok(()) => return Ok(TempDir { path, keep }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(io::ErrorKind::AlreadyExists, "no unused temporary directory name found"))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn write_file(path: &Path, text: &str) -> Result<(), BuildError> {
    fs::write(path, text)
        .map_err(|e| BuildError::Io { what: format!("cannot write {}", path.display()), source: e })
}

/// Quote an argument for display only (the command is never run through a shell).
fn display_arg(a: &OsStr) -> String {
    let s = a.to_string_lossy();
    if s.is_empty() || s.contains(char::is_whitespace) {
        format!("'{s}'")
    } else {
        s.into_owned()
    }
}

/// The executable name `build` uses when `-o` is not given: the source file's stem, in the
/// current directory (with `.exe` on Windows).
pub fn default_output(source_path: &Path) -> PathBuf {
    let stem = source_path.file_stem().filter(|s| !s.is_empty()).unwrap_or_else(|| OsStr::new("a"));
    let mut name = OsString::from(stem);
    if cfg!(windows) {
        name.push(".exe");
    }
    PathBuf::from(name)
}

/// Compile Ubel source to an executable. Front end and emitter errors are reported before the
/// compiler is looked for, so a program that is wrong is never blamed on a missing clang.
pub fn build(source: &str, opts: &BuildOptions) -> Result<BuildOutput, BuildError> {
    let ir = crate::compile(source)?;
    build_ir(&ir, opts)
}

/// The back half of [`build`]: textual LLVM IR to an executable.
pub fn build_ir(ir: &str, opts: &BuildOptions) -> Result<BuildOutput, BuildError> {
    let compiler = find_compiler(opts.cc.as_deref())?;

    // clang's own message for a missing output directory does not say which path it means.
    if let Some(parent) = opts.output.parent() {
        if !parent.as_os_str().is_empty() && !parent.is_dir() {
            return Err(BuildError::OutputDirMissing(parent.to_path_buf()));
        }
    }

    let mut base = opts.tmp_base.clone().unwrap_or_else(std::env::temp_dir);
    if base.is_relative() {
        if let Ok(cwd) = std::env::current_dir() {
            base = cwd.join(base);
        }
    }
    let tmp = TempDir::create(&base, opts.keep_temps).map_err(|e| BuildError::Io {
        what: format!("cannot create a temporary directory in {}", base.display()),
        source: e,
    })?;

    let ll = tmp.path.join("program.ll");
    write_file(&ll, ir)?;
    for (name, text) in RUNTIME_FILES {
        write_file(&tmp.path.join(name), text)?;
    }

    // The same command line the equivalence suite uses (scripts/run_equiv.sh).
    let mut args: Vec<OsString> = vec![opts.opt.flag().into()];
    args.extend(opts.extra_flags.iter().map(OsString::from));
    args.push("-Wno-override-module".into());
    args.push("-I".into());
    args.push(tmp.path.clone().into());
    args.push(ll.into());
    args.push(tmp.path.join("ubel_rt.c").into());
    args.push(tmp.path.join("ubel_main.c").into());
    args.push("-o".into());
    args.push(opts.output.clone().into());

    let mut command = display_arg(compiler.path.as_os_str());
    for a in &args {
        command.push(' ');
        command.push_str(&display_arg(a));
    }

    let out = Command::new(&compiler.path).args(&args).output().map_err(|e| BuildError::Io {
        what: format!("cannot run {}", compiler.path.display()),
        source: e,
    })?;
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let kept_dir = if opts.keep_temps { Some(tmp.path.clone()) } else { None };

    if !out.status.success() {
        return Err(BuildError::CompilerFailed { command, status: out.status.to_string(), stderr, kept_dir });
    }
    Ok(BuildOutput { output: opts.output.clone(), compiler, command, warnings: stderr, kept_dir })
}

// ---------------------------------------------------------------------------------------
// Unit tests (no real clang needed: the compiler search is checked with fake compilers)
// ---------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clang_version_lines() {
        let cases: [(&str, Option<(u32, bool)>); 8] = [
            ("Ubuntu clang version 18.1.3 (1ubuntu1)\nTarget: x86_64-pc-linux-gnu", Some((18, false))),
            ("clang version 15.0.7", Some((15, false))),
            ("Debian clang version 14.0.6", Some((14, false))),
            ("clang version 20.1.0-rc1", Some((20, false))),
            ("Apple clang version 15.0.0 (clang-1500.3.9.4)", Some((15, true))),
            ("Homebrew clang version 17.0.6", Some((17, false))),
            ("gcc (Ubuntu 13.2.0-23ubuntu4) 13.2.0", None),
            ("clang version abc", None),
        ];
        for (text, want) in cases {
            let got = parse_clang_version(text).map(|v| (v.major, v.apple));
            assert_eq!(got, want, "for {text:?}");
        }
        // the version may be on a later line
        let v = parse_clang_version("some banner\nclang version 16.0.0\n").unwrap();
        assert_eq!((v.major, v.line.as_str()), (16, "clang version 16.0.0"));
    }

    #[test]
    fn opt_levels() {
        for (s, flag) in [("0", "-O0"), ("1", "-O1"), ("2", "-O2"), ("3", "-O3"), ("s", "-Os")] {
            assert_eq!(OptLevel::parse(s).unwrap().flag(), flag);
        }
        assert_eq!(OptLevel::parse("4"), None);
        assert_eq!(OptLevel::parse(""), None);
        assert_eq!(OptLevel::default(), OptLevel::O2);
    }

    #[test]
    fn default_output_name() {
        let expect = if cfg!(windows) { "prog.exe" } else { "prog" };
        assert_eq!(default_output(Path::new("a/b/prog.ubl")), PathBuf::from(expect));
        let expect = if cfg!(windows) { "a.b.exe" } else { "a.b" };
        assert_eq!(default_output(Path::new("a.b.ubl")), PathBuf::from(expect));
    }

    /// All the fake-compiler scenarios are in one test on purpose: writing a script and
    /// running it straight away races with other threads that spawn processes (the kernel can
    /// refuse to run a file that another process still has open for writing), and a single
    /// test keeps this binary from spawning anything concurrently.
    #[cfg(unix)]
    #[test]
    fn compiler_search_order_with_fake_compilers() {
        use std::os::unix::fs::PermissionsExt;

        /// Removes the scratch tree again, also when an assertion below fails.
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }

        let root = std::env::temp_dir().join(format!("ubel-driver-unit-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let _cleanup = Cleanup(root.clone());

        let make = |dir: &str, name: &str, version_text: &str| -> PathBuf {
            let d = root.join(dir);
            fs::create_dir_all(&d).unwrap();
            let p = d.join(name);
            fs::write(&p, format!("#!/bin/sh\necho '{version_text}'\n")).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        let path_of = |dirs: &[&str]| -> Option<OsString> {
            Some(std::env::join_paths(dirs.iter().map(|d| root.join(d))).unwrap())
        };
        let name_of = |r: Result<CompilerInfo, BuildError>| -> String {
            r.unwrap().path.file_name().unwrap().to_string_lossy().into_owned()
        };

        // (a) the newest tested versioned name beats a plain `clang`, whatever its version
        make("a", "clang-18", "clang version 18.1.3");
        make("a", "clang", "clang version 20.0.0");
        assert_eq!(name_of(find_compiler_with(None, None, path_of(&["a"]).as_deref())), "clang-18");

        // (b) a missing versioned name falls through to the next; the plain one is last
        make("b", "clang-15", "clang version 15.0.7");
        make("b", "clang", "clang version 14.0.0");
        assert_eq!(name_of(find_compiler_with(None, None, path_of(&["b"]).as_deref())), "clang-15");

        // (c) only a plain clang that is too old: reported as too old, with its version
        make("c", "clang", "Debian clang version 14.0.6");
        match find_compiler_with(None, None, path_of(&["c"]).as_deref()) {
            Err(BuildError::CompilerTooOld { major, version_line, .. }) => {
                assert_eq!(major, 14);
                assert!(version_line.contains("14.0.6"));
            }
            other => panic!("expected CompilerTooOld, got {other:?}"),
        }

        // (d) an unusable candidate is skipped when a later one works
        make("d", "clang-16", "gcc (GNU) 13.2.0");
        make("d", "clang-15", "clang version 15.0.0");
        assert_eq!(name_of(find_compiler_with(None, None, path_of(&["d"]).as_deref())), "clang-15");

        // (e) nothing found
        fs::create_dir_all(root.join("empty")).unwrap();
        match find_compiler_with(None, None, path_of(&["empty"]).as_deref()) {
            Err(BuildError::NoCompiler { tried }) => assert_eq!(tried.len(), AUTO_CANDIDATES.len()),
            other => panic!("expected NoCompiler, got {other:?}"),
        }
        match find_compiler_with(None, None, None) {
            Err(BuildError::NoCompiler { .. }) => {}
            other => panic!("expected NoCompiler without a PATH, got {other:?}"),
        }

        // (f) --cc wins over UBEL_CC and over the PATH; and it is never replaced when unusable
        let custom = make("f", "my-cc", "clang version 17.0.1");
        let env_cc = make("f", "env-cc", "clang version 16.0.1");
        let r = find_compiler_with(Some(&custom), Some(env_cc.as_os_str()), path_of(&["a"]).as_deref());
        assert_eq!(name_of(r), "my-cc");
        let bad = make("f", "bad-cc", "clang version 13.0.0");
        assert!(matches!(
            find_compiler_with(Some(&bad), None, path_of(&["a"]).as_deref()),
            Err(BuildError::CompilerTooOld { major: 13, .. })
        ));

        // (g) UBEL_CC wins over the PATH; an empty one is ignored
        assert_eq!(name_of(find_compiler_with(None, Some(env_cc.as_os_str()), path_of(&["a"]).as_deref())), "env-cc");
        assert_eq!(name_of(find_compiler_with(None, Some(OsStr::new("")), path_of(&["a"]).as_deref())), "clang-18");

        // (h) a named compiler that is missing says where the name came from; a bare name is
        //     looked up on the PATH
        match find_compiler_with(Some(Path::new("/nonexistent/clang")), None, None) {
            Err(BuildError::ExplicitNotFound { source: "--cc", name }) => assert_eq!(name, "/nonexistent/clang"),
            other => panic!("expected ExplicitNotFound for --cc, got {other:?}"),
        }
        match find_compiler_with(None, Some(OsStr::new("no-such-cc")), path_of(&["a"]).as_deref()) {
            Err(BuildError::ExplicitNotFound { source: "UBEL_CC", .. }) => {}
            other => panic!("expected ExplicitNotFound for UBEL_CC, got {other:?}"),
        }
        assert_eq!(
            name_of(find_compiler_with(Some(Path::new("my-cc")), None, path_of(&["f"]).as_deref())),
            "my-cc"
        );

        // (i) Apple's numbering is not held to the minimum; (j) a non-clang is refused
        make("i", "clang", "Apple clang version 14.0.3 (clang-1403.0.22.14.1)");
        assert!(find_compiler_with(None, None, path_of(&["i"]).as_deref()).unwrap().version.apple);
        make("j", "clang", "gcc (Ubuntu 13.2.0-23ubuntu4) 13.2.0");
        assert!(matches!(
            find_compiler_with(None, None, path_of(&["j"]).as_deref()),
            Err(BuildError::CompilerUnusable { .. })
        ));

        // (k) a file without the execute bit is not a candidate
        let d = root.join("k");
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("clang-18"), "#!/bin/sh\necho 'clang version 18.0.0'\n").unwrap();
        assert!(matches!(
            find_compiler_with(None, None, path_of(&["k"]).as_deref()),
            Err(BuildError::NoCompiler { .. })
        ));
    }

    #[test]
    fn compiler_failure_message_is_bounded() {
        let stderr: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let e = BuildError::CompilerFailed {
            command: "clang x".into(),
            status: "exit status: 1".into(),
            stderr,
            kept_dir: None,
        };
        let text = e.to_string();
        assert!(text.contains("| line 0"));
        assert!(text.contains(&format!("| line {}", STDERR_LINES_SHOWN - 1)));
        assert!(!text.contains(&format!("| line {}\n", STDERR_LINES_SHOWN)));
        assert!(text.contains("70 more lines"));
        assert!(text.contains("--keep-temps"));
    }
}
