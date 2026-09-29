mod commands;
mod error;
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
    },

    /// Extract text from PDF
    Extract {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: Option<String>,
        #[arg(long)]
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
    Split {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output_dir: String,
        #[arg(long)]
        ranges: Option<String>,
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
        #[arg(long)]
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
        password: String,
        #[arg(long)]
        output: String,
    },

    /// Compress PDF (reduce file size)
    Compress {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
        #[arg(long, default_value = "balanced")]
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
    #[command(
        name = "web-to-pdf",
        long_about = "Render a URL, HTML file, SVG, or image to PDF using headless Microsoft Edge.\n\n\
            Edge is auto-detected; override with --edge-path if installed elsewhere.\n\n\
            Examples:\n  \
            pdf-cli web-to-pdf --input https://example.com --output page.pdf\n  \
            pdf-cli web-to-pdf --input logo.svg --output logo.pdf\n  \
            pdf-cli web-to-pdf --input photo.png --output photo.pdf --landscape\n  \
            pdf-cli web-to-pdf --input a.html --output a.pdf --edge-path \"D:\\Edge\\msedge.exe\""
    )]
    WebToPdf {
        /// URL (http/https), or a local .html/.svg/image path
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
        /// Explicit path to msedge.exe
        #[arg(long)]
        edge_path: Option<String>,
        /// Print in landscape orientation
        #[arg(long, default_value_t = false)]
        landscape: bool,
    },

    /// Render PDF pages to PNG/JPEG images
    #[command(
        name = "pdf-to-image",
        long_about = "Render each PDF page to a PNG or JPEG image.\n\n\
            Requires the pdfium dynamic library (pdfium.dll). If not found on the system,\n\
            download it from https://github.com/bblanchon/pdfium-binaries/releases and pass\n\
            --pdfium-path <path to pdfium.dll>.\n\n\
            Examples:\n  \
            pdf-cli pdf-to-image --input doc.pdf --output-dir ./imgs\n  \
            pdf-cli pdf-to-image --input doc.pdf --output-dir ./imgs --format jpeg --width 1600\n  \
            pdf-cli pdf-to-image --input doc.pdf --output-dir ./imgs --pages 1-3\n  \
            pdf-cli pdf-to-image --input doc.pdf --output-dir ./imgs --pdfium-path C:\\pdfium\\pdfium.dll"
    )]
    PdfToImage {
        #[arg(long)]
        input: String,
        #[arg(long)]
        output_dir: String,
        /// png (default) or jpeg
        #[arg(long, default_value = "png")]
        format: String,
        /// Target render width in pixels
        #[arg(long, default_value_t = 2000)]
        width: u32,
        /// Page range (default: all)
        #[arg(long)]
        pages: Option<String>,
        /// Explicit path to pdfium.dll
        #[arg(long)]
        pdfium_path: Option<String>,
    },

    /// Combine images/PDFs into an A4 sheet, 2 items stacked vertically per page
    #[command(
        name = "invoice",
        long_about = "Place multiple images and/or PDF files onto A4 pages, 2 per page (top/bottom).\n\n\
            Each item is auto-scaled to fit its half of the page and centered.\n\
            For PDF inputs, only the first page of each file is used.\n\n\
            Examples:\n  \
            pdf-cli invoice --input \"inv1.jpg,inv2.jpg\" --output merged.pdf\n  \
            pdf-cli invoice --input \"a.pdf,b.png,c.pdf\" --output bundle.pdf"
    )]
    Invoice {
        /// Comma-separated list of image or PDF paths
        #[arg(long)]
        input: String,
        #[arg(long)]
        output: String,
    },
}

fn main() {
    let cli = Cli::parse();
    let json = cli.json;
    let dry_run = cli.dry_run;

    let result: Result<CommandResult, error::PdfError> = match cli.command {
        Commands::Convert {
            input,
            output,
            page_size,
            font_size,
        } => commands::convert::run(&input, &output, &page_size, font_size, dry_run),
        Commands::Extract {
            input,
            output,
            pages,
        } => commands::extract::run(&input, output.as_deref(), pages.as_deref()),
        Commands::Merge { input, output } => commands::pages::merge(&input, &output, dry_run),
        Commands::Split {
            input,
            output_dir,
            ranges,
        } => commands::pages::split(&input, &output_dir, ranges.as_deref(), dry_run),
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
        Commands::Decrypt {
            input,
            password,
            output,
        } => commands::security::decrypt(&input, &password, &output, dry_run),
        Commands::Compress {
            input,
            output,
            level,
        } => commands::optimize::compress(&input, &output, &level, dry_run),
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
    };

    match result {
        Ok(r) => {
            r.print(json);
            std::process::exit(0);
        }
        Err(e) => {
            if json {
                println!("{}", serde_json::to_string_pretty(&e.to_json()).unwrap());
            } else {
                eprintln!("Error: {}", e);
            }
            std::process::exit(1);
        }
    }
}
