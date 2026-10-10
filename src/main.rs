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
        #[arg(long)]
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
        #[arg(long)]
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

    /// OCR scanned PDFs via an ocr-rust HTTP service
    #[command(
        long_about = "Run OCR on scanned PDF pages via an external ocr-rust HTTP service.\n\n\
            Requires pdfium.dll (for rendering pages) and a running ocr-rust service.\n\n\
            Start the service first:\n  \
            cd F:\\rust\\ocr-rust; .\\target\\release\\ocr-rust.exe   # port 18004\n\n\
            Examples:\n  \
            pdf-cli ocr --input scan.pdf --output text.txt\n  \
            pdf-cli ocr --input scan.pdf --pages 1-5 --service http://localhost:18004 --json"
    )]
    Ocr {
        #[arg(long)]
        input: String,
        /// Output text file (default: stdout)
        #[arg(long)]
        output: Option<String>,
        /// OCR service base URL
        #[arg(long, default_value = "http://localhost:18004")]
        service: String,
        /// Render width in pixels before OCR
        #[arg(long, default_value_t = 2000)]
        width: u32,
        /// Page range (default: all)
        #[arg(long)]
        pages: Option<String>,
        /// Explicit path to pdfium.dll
        #[arg(long)]
        pdfium_path: Option<String>,
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
        Commands::Pagenum {
            input,
            output,
            position,
            format,
            start,
            font_size,
        } => commands::pagenum::run(
            &input, &output, &position, &format, start, font_size, dry_run,
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
        Commands::Ocr {
            input,
            output,
            service,
            width,
            pages,
            pdfium_path,
        } => commands::ocr::run(
            &input,
            output.as_deref(),
            &service,
            width,
            pages.as_deref(),
            pdfium_path.as_deref(),
            dry_run,
        ),
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
