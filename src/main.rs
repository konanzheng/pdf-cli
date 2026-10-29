mod commands;
mod error;
mod font;
mod io;
mod output;

use clap::{Parser, Subcommand};
use output::CommandResult;

#[derive(Parser)]
#[command(
    name = "pdf-cli",
    version,
    about = "AI-native PDF toolkit — non-interactive, JSON-first",
    long_about = "A PDF toolkit designed for AI agent interaction.\n\n\
        All commands are non-interactive. Use --json for machine-readable output.\n\
        Use --dry-run to preview destructive operations."
)]
struct Cli {
    /// Output machine-readable JSON
    #[arg(long, global = true)]
    json: bool,

    /// Preview without writing (for destructive ops)
    #[arg(long, global = true)]
    dry_run: bool,

    /// Suppress progress messages on stderr
    #[arg(long, global = true, default_value_t = false)]
    quiet: bool,

    /// Password for encrypted input PDFs (applies to every command)
    #[arg(long, global = true)]
    password: Option<String>,

    /// Emit one JSON object per line (for batch / multi-file output)
    #[arg(long, global = true, default_value_t = false)]
    json_lines: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Convert images/Markdown/text to PDF
    Convert {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
        #[arg(long, default_value = "a4")]
        page_size: String,
        #[arg(long, default_value_t = 11.0)]
        font_size: f32,
        /// Embed a TrueType font (e.g. for Chinese text); omit for built-in Helvetica
        #[arg(long)]
        font: Option<String>,
    },

    /// Extract text from PDF
    Extract {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: Option<String>,
        #[arg(long, allow_negative_numbers = true)]
        pages: Option<String>,
    },

    /// Merge multiple PDFs
    Merge {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
    },

    /// Split PDF into separate files
    #[command(
        long_about = "Split a PDF into several files.\n\n\
            Choose one grouping strategy:\n  \
            --ranges \"1-3,4-6\"  explicit groups (comma separated)\n  \
            --every 2           fixed-size chunks of N pages\n  \
            --odd-even          two files: odd pages, then even pages\n\n\
            With no flag every page becomes its own file.\n\n\
            Examples:\n  \
            pdf-cli split --input in.pdf --output-dir out --every 10\n  \
            pdf-cli split --input in.pdf --output-dir out --odd-even"
    )]
    Split {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output_dir: String,
        /// Explicit page groups, e.g. "1-3,4-6"
        #[arg(long)]
        ranges: Option<String>,
        /// Fixed chunk size: every N pages become one file
        #[arg(long)]
        every: Option<u32>,
        /// Split into odd pages and even pages
        #[arg(long, default_value_t = false)]
        odd_even: bool,
    },

    /// Delete specific pages
    DeletePages {
        #[arg(long)]
        input: String,
        #[arg(long)]
        pages: String,
        #[arg(long)]
        output: String,
    },

    /// Rotate pages
    Rotate {
        #[arg(long)]
        input: String,
        #[arg(long)]
        angle: i32,
        #[arg(long, allow_negative_numbers = true)]
        pages: Option<String>,
        #[arg(long)]
        output: String,
    },

    /// Add text watermark to PDF
    WatermarkAdd {
        #[arg(long)]
        input: String,
        #[arg(long)]
        text: String,
        #[arg(long)]
        output: String,
        #[arg(long, default_value_t = 0.2)]
        opacity: f32,
        #[arg(long, default_value_t = 45.0)]
        rotation: f32,
        #[arg(long, default_value_t = 60.0)]
        font_size: f32,
    },

    /// Remove burned-in image watermarks from scanned/raster pages (best-effort)
    #[command(name = "watermark-remove")]
    WatermarkRemove {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
        /// Luminance threshold (0-255): pixels at/above this are treated as watermark/background
        #[arg(long, default_value_t = 212)]
        threshold: u8,
        /// Max color saturation (0-255): only low-saturation pixels are whited out (protects colored seals)
        #[arg(long, default_value_t = 12)]
        max_sat: u8,
        /// Page range, e.g. 1-3,5 (default: all)
        #[arg(long, allow_negative_numbers = true)]
        pages: Option<String>,
        /// JPEG re-encode quality (1-100)
        #[arg(long, default_value_t = 82)]
        quality: u8,
    },

    /// Encrypt PDF with password
    Encrypt {
        #[arg(long)]
        input: String,
        #[arg(long)]
        user_password: Option<String>,
        #[arg(long)]
        owner_password: Option<String>,
        #[arg(long)]
        output: String,
    },

    /// Remove password protection
    Decrypt {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
    },

    /// Compress PDF (reduce file size)
    Compress {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
        #[arg(long, default_value = "medium")]
        level: String,
    },

    /// Show PDF metadata and structure info
    Info {
        #[arg(long)]
        input: String,
    },

    /// Deep structural analysis: pages, sizes, images, fonts, warnings
    Doctor {
        #[arg(long)]
        input: String,
    },

    /// Print a web page / HTML / SVG / image to PDF via headless Edge
    #[command(name = "web-to-pdf")]
    WebToPdf {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
        #[arg(long)]
        edge_path: Option<String>,
        #[arg(long, default_value_t = false)]
        landscape: bool,
    },

    /// Render PDF pages to PNG/JPEG images
    #[command(name = "pdf-to-image")]
    PdfToImage {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output_dir: String,
        #[arg(long, default_value = "png")]
        format: String,
        #[arg(long, default_value_t = 2000)]
        width: u32,
        #[arg(long, allow_negative_numbers = true)]
        pages: Option<String>,
        #[arg(long)]
        pdfium_path: Option<String>,
    },

    /// Combine images/PDFs into an A4 sheet, 2 items stacked vertically per page
    Invoice {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
    },

    /// Cover matching text and write a replacement in its place
    #[command(
        name = "redact",
        long_about = "Find text by searching and replace it in place: cover the found \
            rectangle with an opaque box, then draw the new text on top.\n\n\
            Built for bulk edits — the same wording repeated across many pages, where each \
            copy differs by a name, a date or an amount.\n\n\
            IMPORTANT: the original text stays inside the content stream. The cover hides it \
            from view, but `extract`, `search` and copy-paste still reach it. This is a \
            presentation fix, not a security control: never rely on it to redact secrets for \
            publication. To make removal irreversible, rasterise the pages instead.\n\n\
            Requires pdfium, because the rectangle must be measured — there is no fallback.\n\n\
            Examples:\n  \
            pdf-cli redact --input in.pdf --output out.pdf --find \"Acme Corp\" --replace \"Beta Ltd\" --dry-run --json\n  \
            pdf-cli redact --input in.pdf --output out.pdf --find \"2024-0042\" --replace \"2025-0099\"\n  \
            pdf-cli redact --input in.pdf --output out.pdf --find \"草稿\" --replace \"定稿\" --font C:\\Windows\\Fonts\\msyh.ttc"
    )]
    Redact {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
        /// Text to find (literal)
        #[arg(long, value_name = "TEXT")]
        find: String,
        /// Replacement text
        #[arg(long, value_name = "TEXT")]
        replace: String,
        /// Page range (default: all)
        #[arg(long, allow_negative_numbers = true)]
        pages: Option<String>,
        /// Cover fill colour as "r,g,b" with each component 0-1
        #[arg(long, default_value = "1 1 1")]
        cover_color: String,
        /// Path to a .ttf/.ttc font (required for CJK replacements; auto-detected by default)
        #[arg(long)]
        font: Option<String>,
        /// Grow the cover box by this many points on every side
        #[arg(long, default_value_t = 0.5)]
        padding: f32,
        /// Explicit path to pdfium.dll
        #[arg(long)]
        pdfium_path: Option<String>,
    },

    /// Draw text or cover boxes at exact page coordinates
    #[command(
        name = "overlay",
        long_about = "Place text and optional cover boxes at coordinates you specify.\n\n\
            The manual counterpart to `redact`: use it for fixed-layout templates, or for \
            scanned pages that have no text layer to search.\n\n\
            `--text` takes PAGE:key=value,... items separated by ';'. PAGE is N, N-M or *.\n\
            Keys: x, y (points, origin bottom-left, required), text, size, w, h, cover (r/g/b).\n\n\
            Examples:\n  \
            pdf-cli overlay --input in.pdf --output out.pdf --text \"1:x=72,y=700,text=Hello,size=12\" --json\n  \
            pdf-cli overlay --input in.pdf --output out.pdf --text \"*:x=72,y=760,text=DRAFT\"\n  \
            pdf-cli overlay --input in.pdf --output out.pdf --text \"2-4:x=100,y=600,w=120,h=14,cover=1/1/1,text=Acme\""
    )]
    Overlay {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
        /// Placement spec: "PAGE:x=..,y=..,text=..;PAGE2:..."
        #[arg(long, value_name = "SPEC")]
        text: String,
        /// Path to a .ttf/.ttc font (required for CJK text; auto-detected by default)
        #[arg(long)]
        font: Option<String>,
    },

    /// Add page numbers
    #[command(name = "pagenum")]
    Pagenum {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
        /// bottom-center (default) | bottom-left | bottom-right | top-center | top-left | top-right
        #[arg(long, default_value = "bottom-center")]
        position: String,
        /// Template with {n} and {total}, e.g. "第 {n} 页 / 共 {total} 页"
        #[arg(long, default_value = "{n} / {total}")]
        format: String,
        /// First displayed page number
        #[arg(long, default_value_t = 1)]
        start: u32,
        #[arg(long, default_value_t = 10.0)]
        font_size: f32,
        /// Path to a .ttf/.ttc font (required for CJK templates; auto-detected by default)
        #[arg(long)]
        font: Option<String>,
    },

    /// Edit document metadata
    Meta {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        author: Option<String>,
        #[arg(long)]
        subject: Option<String>,
        #[arg(long)]
        keywords: Option<String>,
        #[arg(long)]
        creator: Option<String>,
        #[arg(long)]
        producer: Option<String>,
    },

    /// Extract embedded images (original format, no re-render)
    #[command(name = "extract-images")]
    ExtractImages {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output_dir: String,
        /// Skip images whose larger side is below this (px)
        #[arg(long, default_value_t = 0)]
        min_size: u32,
    },

    /// Search text in a PDF: regex/literal, page + bbox + context, JSON-first
    #[command(
        long_about = "Search for text across a PDF and return machine-checkable hits.\n\n\
            Each hit reports the page, bounding box (PDF points, origin top-left), the matched\n\
            text, and surrounding context. Falls back to plain-text search when pdfium is\n\
            unavailable; bbox/page geometry then comes back as null.\n\n\
            Examples:\n  \
            pdf-cli search --input doc.pdf --pattern \"invoice\" --json\n  \
            pdf-cli search --input doc.pdf --pattern \"\\d{4}-\\d{2}-\\d{2}\" --regex --json\n  \
            pdf-cli search --input doc.pdf --pattern \"第.章\" --regex --count-only --json"
    )]
    Search {
        #[arg(long)]
        input: String,
        /// Text to find (literal, unless --regex)
        #[arg(long)]
        pattern: String,
        /// Treat the pattern as a regular expression
        #[arg(long, default_value_t = false)]
        regex: bool,
        /// Case-insensitive match
        #[arg(long, default_value_t = false)]
        ignore_case: bool,
        /// Page range (default: all)
        #[arg(long, allow_negative_numbers = true)]
        pages: Option<String>,
        /// Characters of context to include around each hit
        #[arg(long, default_value_t = 40)]
        context: usize,
        /// Only report counts, not individual hits
        #[arg(long, default_value_t = false)]
        count_only: bool,
        /// Stop after this many hits (0 = unlimited)
        #[arg(long, default_value_t = 0)]
        max_results: usize,
        /// Explicit path to pdfium.dll
        #[arg(long)]
        pdfium_path: Option<String>,
    },

    /// Convert PDF to Markdown (headings, lists, tables, image anchors)
    #[command(
        name = "to-markdown",
        long_about = "Convert a PDF into Markdown an LLM can read directly.\n\n\
            Reconstructs headings from font size, joins lines into paragraphs, detects list\n\
            markers and simple tables from text geometry, and emits `<!-- page N -->` markers.\n\n\
            Modes:\n  \
            doc    (default) structure-aware Markdown\n  \
            plain  one paragraph per text line, no heuristics"
    )]
    ToMarkdown {
        #[arg(long)]
        input: String,
        /// Output .md file (default: stdout)
        #[arg(long)]
        output: Option<String>,
        /// Page range (default: all)
        #[arg(long, allow_negative_numbers = true)]
        pages: Option<String>,
        /// doc | plain
        #[arg(long, default_value = "doc")]
        mode: String,
        /// Explicit path to pdfium.dll
        #[arg(long)]
        pdfium_path: Option<String>,
    },

    /// Compare two PDFs by text and/or rendered pixels
    #[command(
        long_about = "Diff two PDFs and report what changed.\n\n\
            text mode compares normalized extracted text per page (fast, exact).\n\
            render mode compares rasterized pages and reports the changed-area ratio.\n\
            both runs text first and falls back to render for pages whose text matches.\n\n\
            Exit code is 0 even when files differ; read details.different / details.pages[].changed."
    )]
    Diff {
        #[arg(long)]
        input: String,
        /// Second PDF to compare against
        #[arg(long)]
        input2: String,
        /// text | render | both
        #[arg(long, default_value = "text")]
        mode: String,
        /// Fraction of differing pixels (0-1) that still counts as "same" in render mode
        #[arg(long, default_value_t = 0.001)]
        threshold: f32,
        /// Directory to write per-page diff masks (render mode)
        #[arg(long)]
        output: Option<String>,
        /// Page range applied to both files (default: all)
        #[arg(long, allow_negative_numbers = true)]
        pages: Option<String>,
        /// Render width in pixels for render mode
        #[arg(long, default_value_t = 1000)]
        width: u32,
        /// Explicit path to pdfium.dll
        #[arg(long)]
        pdfium_path: Option<String>,
    },

    /// Inspect, fill and flatten AcroForm fields
    #[command(
        long_about = "Work with interactive PDF forms.\n\n\
            Actions:\n  \
            list      report every field with name, type, current value, required flag\n  \
            set       fill fields from --values 'name=value;other=value'\n  \
            flatten   bake field appearances into page content (no longer editable)\n\n\
            Examples:\n  \
            pdf-cli form --input f.pdf --action list --json\n  \
            pdf-cli form --input f.pdf --action set --values \"name=Ada;agree=true\" --output out.pdf"
    )]
    Form {
        #[arg(long)]
        input: String,
        /// list | set | flatten
        #[arg(long, default_value = "list")]
        action: String,
        /// Field name filter (comma-separated) for list/set
        #[arg(long)]
        fields: Option<String>,
        /// Values as name=value pairs separated by ';'
        #[arg(long)]
        values: Option<String>,
        /// Output PDF (required for set/flatten)
        #[arg(long)]
        output: Option<String>,
        /// Explicit path to pdfium.dll
        #[arg(long)]
        pdfium_path: Option<String>,
    },

    /// Crop pages to an explicit box or to the content bounding box
    #[command(
        long_about = "Trim page margins by setting the CropBox.\n\n\
            --box takes 'left,bottom,right,top' in PDF points (origin bottom-left).\n\
            --auto scans per-page ink extents and crops to content plus --margin.\n\n\
            Examples:\n  \
            pdf-cli crop --input in.pdf --output out.pdf --auto --margin 10\n  \
            pdf-cli crop --input in.pdf --output out.pdf --box \"0,0,595,842\" --pages 1-3"
    )]
    Crop {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
        /// Explicit crop box: left,bottom,right,top in points
        #[arg(long = "box")]
        box_spec: Option<String>,
        /// Crop to the detected content bounding box
        #[arg(long, default_value_t = false)]
        auto: bool,
        /// Padding in points added around the detected content box (--auto)
        #[arg(long, default_value_t = 0.0)]
        margin: f32,
        /// Page range (default: all)
        #[arg(long, allow_negative_numbers = true)]
        pages: Option<String>,
        /// Path to pdfium.dll (required for --auto)
        #[arg(long)]
        pdfium_path: Option<String>,
    },

    /// Reorder, insert or duplicate pages in one pass
    #[command(
        long_about = "Reorder the pages of a PDF.\n\n\
            --order takes 1-based page numbers, ranges, and repeats, e.g. \"3,1-2,3\".\n\
            --insert takes 'position:file' pairs to splice another PDF in, e.g. \"2:b.pdf\".\n\n\
            Examples:\n  \
            pdf-cli organize --input in.pdf --output out.pdf --order \"3,1-2\"\n  \
            pdf-cli organize --input a.pdf --output out.pdf --insert \"2:b.pdf\""
    )]
    Organize {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
        /// New page order, e.g. "3,1-2" (repeats allowed, omitting drops pages)
        #[arg(long)]
        order: Option<String>,
        /// Insert other PDFs: "position:file[,position:file...]"
        #[arg(long)]
        insert: Option<String>,
    },
}

/// Which commands can send their primary payload to stdout via `--output -`.
///
/// These are the commands whose result is a document or a block of text rather
/// than a status summary, so stdout is a plausible sink for them. Listing them
/// explicitly keeps the `--output -` versus `--json` conflict check honest: a
/// command that cannot stream to stdout is never falsely rejected.
fn writes_payload_to_stdout(cmd: &Commands) -> bool {
    match cmd {
        // `--output` is a required String for these: compare directly.
        Commands::Compress { output, .. }
        | Commands::Decrypt { output, .. }
        | Commands::Encrypt { output, .. }
        | Commands::Rotate { output, .. }
        | Commands::DeletePages { output, .. }
        | Commands::Meta { output, .. }
        | Commands::Pagenum { output, .. }
        | Commands::WatermarkAdd { output, .. }
        | Commands::WatermarkRemove { output, .. }
        | Commands::Crop { output, .. }
        | Commands::Organize { output, .. }
        | Commands::Overlay { output, .. }
        | Commands::Redact { output, .. }
        | Commands::Convert { output, .. }
        | Commands::Merge { output, .. }
        | Commands::Invoice { output, .. } => output == io::STDIO,
        // `--output` is optional for these: absent means "result is the payload".
        Commands::Extract { output, .. } | Commands::ToMarkdown { output, .. } => {
            matches!(output.as_deref(), Some(io::STDIO))
        }
        _ => false,
    }
}

fn main() {
    // clap builds the whole command tree with a deep recursive walk. With 27
    // subcommands that walk overflows the 1 MiB default main-thread stack in a
    // debug build, aborting the process before any command code runs -- even for
    // `--version`. Run the real entry point on a thread with a comfortable stack
    // so the CLI behaves identically in every build profile.
    const STACK_BYTES: usize = 16 * 1024 * 1024;
    let handle = std::thread::Builder::new()
        .stack_size(STACK_BYTES)
        .name("pdf-cli".into())
        .spawn(real_main)
        .expect("failed to spawn the main worker thread");
    // Propagate the worker's fate: a panic already printed its message.
    if handle.join().is_err() {
        std::process::exit(1);
    }
}

fn real_main() {
    let cli = Cli::parse();
    let json = cli.json;
    let dry_run = cli.dry_run;
    let quiet = cli.quiet;
    let json_lines = cli.json_lines;
    let password = cli.password.clone();
    io::set_global_password(password.as_deref());

    // `--output -` streams a command's real payload to stdout. Combined with a
    // machine-output mode, stdout would carry the payload *and* the envelope,
    // producing a stream that is neither valid JSON nor clean text — a silent
    // corruption that is very hard to debug from the caller's side. Reject the
    // combination up front instead of emitting something unparseable.
    if (json || json_lines) && writes_payload_to_stdout(&cli.command) {
        let e = error::PdfError::Operation {
            reason: "`--output -` streams the document to stdout, which conflicts with \
                     `--json`/`--json-lines` writing the result envelope there.\n  \
                     → Drop the machine-output flag, or write to a real file with --output <path>"
                .into(),
        };
        if json_lines {
            match serde_json::to_string(&e.to_json()) {
                Ok(s) => println!("{s}"),
                Err(err) => eprintln!("Failed to serialise error: {err}"),
            }
        } else {
            match serde_json::to_string_pretty(&e.to_json()) {
                Ok(s) => println!("{s}"),
                Err(err) => eprintln!("Failed to serialise error: {err}"),
            }
        }
        std::process::exit(1);
    }

    let result: Result<CommandResult, error::PdfError> = match cli.command {
        Commands::Convert {
            input,
            output,
            page_size,
            font_size,
            font,
        } => commands::convert::run(
            &input,
            &output,
            &page_size,
            font_size,
            font.as_deref(),
            dry_run,
        ),
        Commands::Extract {
            input,
            output,
            pages,
        } => commands::extract::run(&input, output.as_deref(), pages.as_deref(), password.as_deref()),
        Commands::Merge { input, output } => commands::pages::merge(&input, &output, dry_run),
        Commands::Split {
            input,
            output_dir,
            ranges,
            every,
            odd_even,
        } => commands::pages::split(
            &input,
            &output_dir,
            ranges.as_deref(),
            every,
            odd_even,
            dry_run,
        ),
        Commands::DeletePages {
            input,
            pages,
            output,
        } => commands::pages::delete_pages(&input, &pages, &output, dry_run),
        Commands::Rotate {
            input,
            angle,
            pages,
            output,
        } => commands::pages::rotate(&input, angle, pages.as_deref(), &output, dry_run),
        Commands::WatermarkAdd {
            input,
            text,
            output,
            opacity,
            rotation,
            font_size,
        } => commands::watermark::add(
            &input, &text, &output, opacity, rotation, font_size, dry_run,
        ),
        Commands::WatermarkRemove {
            input,
            output,
            threshold,
            max_sat,
            pages,
            quality,
        } => commands::watermark::remove(
            &input,
            &output,
            threshold,
            max_sat,
            pages.as_deref(),
            quality,
            dry_run,
        ),
        Commands::Encrypt {
            input,
            user_password,
            owner_password,
            output,
        } => commands::security::encrypt(
            &input,
            user_password.as_deref(),
            owner_password.as_deref(),
            &output,
            dry_run,
        ),
        Commands::Decrypt { input, output } => match password.as_deref() {
            Some(pwd) => commands::security::decrypt(&input, pwd, &output, dry_run),
            None => Err(error::PdfError::MissingArg {
                arg: "--password (the global flag) is required to decrypt".into(),
                example: "pdf-cli --password secret decrypt --input enc.pdf --output dec.pdf"
                    .into(),
            }),
        },
        Commands::Compress {
            input,
            output,
            level,
        } => commands::optimize::compress(&input, &output, &level, password.as_deref(), dry_run),
        Commands::Info { input } => commands::info::run(&input),
        Commands::Doctor { input } => commands::doctor::run(&input),
        Commands::WebToPdf {
            input,
            output,
            edge_path,
            landscape,
        } => commands::webpdf::run(&input, &output, edge_path.as_deref(), landscape, dry_run),
        Commands::PdfToImage {
            input,
            output_dir,
            format,
            width,
            pages,
            pdfium_path,
        } => commands::pdf2img::run(
            &input,
            &output_dir,
            &format,
            width,
            pages.as_deref(),
            pdfium_path.as_deref(),
            dry_run,
        ),
        Commands::Invoice { input, output } => commands::invoice::run(&input, &output, dry_run),
        Commands::Redact {
            input,
            output,
            find,
            replace,
            pages,
            cover_color,
            font,
            padding,
            pdfium_path,
        } => commands::redact::run(
            &input,
            &output,
            &find,
            &replace,
            pages.as_deref(),
            &cover_color,
            font.as_deref(),
            padding,
            password.as_deref(),
            pdfium_path.as_deref(),
            dry_run,
        ),
        Commands::Overlay {
            input,
            output,
            text,
            font,
        } => commands::overlay::run(
            &input,
            &output,
            &text,
            font.as_deref(),
            password.as_deref(),
            dry_run,
        ),
        Commands::Pagenum {
            input,
            output,
            position,
            format,
            start,
            font_size,
            font,
        } => commands::pagenum::run(
            &input,
            &output,
            &position,
            &format,
            start,
            font_size,
            font.as_deref(),
            dry_run,
        ),
        Commands::Meta {
            input,
            output,
            title,
            author,
            subject,
            keywords,
            creator,
            producer,
        } => commands::meta::run(
            &input,
            &output,
            title.as_deref(),
            author.as_deref(),
            subject.as_deref(),
            keywords.as_deref(),
            creator.as_deref(),
            producer.as_deref(),
            dry_run,
        ),
        Commands::ExtractImages {
            input,
            output_dir,
            min_size,
        } => commands::extract_images::run(&input, &output_dir, min_size, dry_run),
        Commands::Search {
            input,
            pattern,
            regex,
            ignore_case,
            pages,
            context,
            count_only,
            max_results,
            pdfium_path,
        } => commands::search::run(
            commands::search::SearchArgs {
                input: &input,
                pattern: &pattern,
                regex,
                ignore_case,
                pages: pages.as_deref(),
                context,
                count_only,
                max_results,
                password: password.as_deref(),
                pdfium_path: pdfium_path.as_deref(),
            },
            dry_run,
        ),
        Commands::ToMarkdown {
            input,
            output,
            pages,
            mode,
            pdfium_path,
        } => commands::to_markdown::run(
            commands::to_markdown::ToMarkdownArgs {
                input: &input,
                output: output.as_deref(),
                pages: pages.as_deref(),
                mode: &mode,
                password: password.as_deref(),
                pdfium_path: pdfium_path.as_deref(),
            },
            dry_run,
        ),
        Commands::Diff {
            input,
            input2,
            mode,
            threshold,
            output,
            pages,
            width,
            pdfium_path,
        } => commands::diffcmd::run(
            commands::diffcmd::DiffArgs {
                input: &input,
                input2: &input2,
                mode: &mode,
                threshold,
                output: output.as_deref(),
                pages: pages.as_deref(),
                width,
                password: password.as_deref(),
                pdfium_path: pdfium_path.as_deref(),
            },
            dry_run,
        ),
        Commands::Form {
            input,
            action,
            fields,
            values,
            output,
            pdfium_path,
        } => commands::form::run(
            commands::form::FormArgs {
                input: &input,
                action: &action,
                fields: fields.as_deref(),
                values: values.as_deref(),
                output: output.as_deref(),
                password: password.as_deref(),
                pdfium_path: pdfium_path.as_deref(),
            },
            dry_run,
        ),
        Commands::Crop {
            input,
            output,
            box_spec,
            auto,
            margin,
            pages,
            pdfium_path,
        } => commands::crop::run(
            commands::crop::CropArgs {
                input: &input,
                output: &output,
                box_spec: box_spec.as_deref(),
                auto,
                margin,
                pages: pages.as_deref(),
                password: password.as_deref(),
                pdfium_path: pdfium_path.as_deref(),
            },
            dry_run,
        ),
        Commands::Organize {
            input,
            output,
            order,
            insert,
        } => commands::organize::run(
            commands::organize::OrganizeArgs {
                input: &input,
                output: &output,
                order: order.as_deref(),
                insert: insert.as_deref(),
                password: password.as_deref(),
            },
            dry_run,
        ),
    };

    match result {
        Ok(r) => {
            r.print(json, quiet, json_lines);
            std::process::exit(0);
        }
        Err(e) => {
            if json || json_lines {
                let payload = e.to_json();
                let rendered = if json_lines {
                    serde_json::to_string(&payload)
                } else {
                    serde_json::to_string_pretty(&payload)
                };
                match rendered {
                    Ok(s) => println!("{s}"),
                    Err(err) => eprintln!("Failed to serialise error: {err}"),
                }
            } else {
                eprintln!("Error: {}", e);
            }
            std::process::exit(1);
        }
    }
}
