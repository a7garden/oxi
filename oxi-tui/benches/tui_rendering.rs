//! Benchmarks for TUI rendering primitives.
//!
//! Measures performance of:
//! - Surface creation and clearing
//! - Surface write_string (the most common hot path)
//! - Surface diff_from (dirty tracking)
//! - Renderer SGR generation (ANSI escape code creation)
//! - Renderer render_to_string (full render without terminal I/O)

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use oxi_tui::{Cell, CellBuilder, Color, Surface};

// ---------------------------------------------------------------------------
// Surface benchmarks
// ---------------------------------------------------------------------------

fn bench_surface_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("surface_creation");

    for (w, h) in [(80u16, 24u16), (120, 40), (200, 60)] {
        let label = format!("{}x{}", w, h);
        let cell_count = (w as u64) * (h as u64);
        group.throughput(Throughput::Elements(cell_count));
        group.bench_with_input(BenchmarkId::new("new", &label), &(w, h), |b, &(w, h)| {
            b.iter(|| Surface::new(black_box(w), black_box(h)));
        });
    }

    group.finish();
}

fn bench_surface_clear(c: &mut Criterion) {
    let mut group = c.benchmark_group("surface_clear");

    for (w, h) in [(80u16, 24u16), (120, 40), (200, 60)] {
        let label = format!("{}x{}", w, h);
        let cell_count = (w as u64) * (h as u64);
        group.throughput(Throughput::Elements(cell_count));
        group.bench_with_input(BenchmarkId::new("clear", &label), &(w, h), |b, &(w, h)| {
            let surface = Surface::new(w, h);
            b.iter(|| {
                let mut s = surface.clone();
                s.clear();
                black_box(&s);
            });
        });
    }

    group.finish();
}

fn bench_surface_write_string(c: &mut Criterion) {
    let mut group = c.benchmark_group("surface_write_string");

    let short = "Hello, world!";
    let medium = "The quick brown fox jumps over the lazy dog. This is a test string.";
    let long: String = std::iter::repeat("abcde ").take(20).collect(); // 120 chars

    group.throughput(Throughput::Bytes(short.len() as u64));
    group.bench_function("short_13chars", |b| {
        let mut surface = Surface::new(120, 40);
        b.iter(|| {
            surface.write_string(black_box(0), black_box(0), black_box(short));
        });
    });

    group.throughput(Throughput::Bytes(medium.len() as u64));
    group.bench_function("medium_70chars", |b| {
        let mut surface = Surface::new(120, 40);
        b.iter(|| {
            surface.write_string(black_box(0), black_box(0), black_box(medium));
        });
    });

    group.throughput(Throughput::Bytes(long.len() as u64));
    group.bench_function("long_120chars", |b| {
        let mut surface = Surface::new(120, 40);
        b.iter(|| {
            surface.write_string(black_box(0), black_box(0), black_box(&long));
        });
    });

    group.finish();
}

fn bench_surface_fill(c: &mut Criterion) {
    let mut group = c.benchmark_group("surface_fill");
    let cell_count = 120u64 * 40;

    group.throughput(Throughput::Elements(cell_count));
    group.bench_function("120x40_default", |b| {
        b.iter(|| {
            let mut s = Surface::new(120, 40);
            s.fill(Cell::new('X'));
            black_box(&s);
        });
    });

    group.finish();
}

fn bench_surface_diff(c: &mut Criterion) {
    let mut group = c.benchmark_group("surface_diff");

    for (w, h) in [(80u16, 24u16), (120, 40)] {
        let label = format!("{}x{}", w, h);
        let cell_count = (w as u64) * (h as u64);

        // Create two surfaces with ~10% cells different
        let a = Surface::new(w, h);
        let mut b = Surface::new(w, h);
        let changes = (w as usize * h as usize) / 10;
        for i in 0..changes {
            let row = (i % (h as usize)) as u16;
            let col = (i / (h as usize)) as u16;
            if col < w {
                b.set(row, col, Cell::new('X'));
            }
        }

        group.throughput(Throughput::Elements(cell_count));
        group.bench_with_input(
            BenchmarkId::new("diff_10pct", &label),
            &(a, b),
            |bencher, (a, b)| {
                bencher.iter(|| {
                    let mut s = a.clone();
                    s.diff_from(black_box(b));
                    black_box(&s);
                });
            },
        );
    }

    group.finish();
}

// ---------------------------------------------------------------------------
// Renderer benchmarks (render to buffer, not stdout)
// ---------------------------------------------------------------------------

/// A buffer-rendering renderer that collects ANSI output into a String.
/// This lets us benchmark the SGR computation without needing a real terminal.
struct BenchRenderer {
    output: String,
    current_sgr_state: (bool, bool, bool, bool, bool, Option<Color>, Option<Color>),
}

impl BenchRenderer {
    fn new() -> Self {
        Self {
            output: String::with_capacity(8192),
            current_sgr_state: (false, false, false, false, false, None, None),
        }
    }

    /// Render a full surface to the internal buffer.
    fn render_to_buffer(&mut self, surface: &Surface) {
        self.output.clear();

        for row in 0..surface.height() {
            for col in 0..surface.width() {
                if let Some(cell) = surface.get(row, col) {
                    // Move cursor
                    self.output.push_str(&format!("\x1b[{};{}H", row + 1, col + 1));

                    // Apply SGR if changed
                    let new_state = (
                        cell.attrs.bold,
                        cell.attrs.italic,
                        cell.attrs.underline,
                        cell.attrs.strikethrough,
                        cell.attrs.reversed,
                        Some(cell.fg),
                        Some(cell.bg),
                    );

                    if new_state != self.current_sgr_state {
                        self.current_sgr_state = new_state;
                        self.output.push_str("\x1b[0m");
                        self.apply_sgr_inline(cell);
                    }

                    self.output.push(cell.char);
                }
            }
        }
    }

    fn apply_sgr_inline(&mut self, cell: &Cell) {
        let mut buf = String::with_capacity(32);
        buf.push_str("\x1b[");

        let mut first = true;

        let mut push_code = |buf: &mut String, code: &str, first: &mut bool| {
            if !*first { buf.push(';'); }
            *first = false;
            buf.push_str(code);
        };

        if cell.attrs.bold { push_code(&mut buf, "1", &mut first); }
        if cell.attrs.italic { push_code(&mut buf, "3", &mut first); }
        if cell.attrs.underline { push_code(&mut buf, "4", &mut first); }
        if cell.attrs.strikethrough { push_code(&mut buf, "9", &mut first); }

        match cell.fg {
            Color::Default => {}
            Color::Black => push_code(&mut buf, "30", &mut first),
            Color::Red => push_code(&mut buf, "31", &mut first),
            Color::Green => push_code(&mut buf, "32", &mut first),
            Color::Yellow => push_code(&mut buf, "33", &mut first),
            Color::Blue => push_code(&mut buf, "34", &mut first),
            Color::Magenta => push_code(&mut buf, "35", &mut first),
            Color::Cyan => push_code(&mut buf, "36", &mut first),
            Color::White => push_code(&mut buf, "37", &mut first),
            Color::Indexed(n) => {
                push_code(&mut buf, "38", &mut first);
                push_code(&mut buf, "5", &mut first);
                let s = n.to_string();
                push_code(&mut buf, &s, &mut first);
            }
            Color::Rgb(r, g, b) => {
                push_code(&mut buf, "38", &mut first);
                push_code(&mut buf, "2", &mut first);
                let rs = r.to_string();
                push_code(&mut buf, &rs, &mut first);
                let gs = g.to_string();
                push_code(&mut buf, &gs, &mut first);
                let bs = b.to_string();
                push_code(&mut buf, &bs, &mut first);
            }
        }

        buf.push('m');
        self.output.push_str(&buf);
    }
}

fn bench_render_full(c: &mut Criterion) {
    let mut group = c.benchmark_group("render_full");

    for (w, h) in [(80u16, 24u16), (120, 40)] {
        let label = format!("{}x{}", w, h);
        let cell_count = (w as u64) * (h as u64);

        // Create a surface with varied content
        let mut surface = Surface::new(w, h);
        for row in 0..h {
            for col in 0..w {
                let ch = match (row + col) % 4 {
                    0 => 'A',
                    1 => 'b',
                    2 => '█',
                    _ => ' ',
                };
                let fg = match col % 5 {
                    0 => Color::Default,
                    1 => Color::Green,
                    2 => Color::Cyan,
                    3 => Color::Indexed(214),
                    _ => Color::Rgb(200, 100, 50),
                };
                surface.set(row, col, Cell::new(ch).with_fg(fg));
            }
        }

        group.throughput(Throughput::Elements(cell_count));
        group.bench_with_input(
            BenchmarkId::new("render_to_buffer", &label),
            &surface,
            |b, surface| {
                let mut renderer = BenchRenderer::new();
                b.iter(|| {
                    renderer.render_to_buffer(black_box(surface));
                    black_box(&renderer.output);
                });
            },
        );
    }

    group.finish();
}

fn bench_render_uniform(c: &mut Criterion) {
    let mut group = c.benchmark_group("render_uniform");

    // All-same cells: SGR only changes once → best case for renderer
    let surface = Surface::new(120, 40);
    let cell_count: u64 = 120 * 40;

    group.throughput(Throughput::Elements(cell_count));
    group.bench_function("120x40_all_default", |b| {
        let mut renderer = BenchRenderer::new();
        b.iter(|| {
            renderer.render_to_buffer(black_box(&surface));
            black_box(&renderer.output);
        });
    });

    group.finish();
}

fn bench_cell_to_ansi(c: &mut Criterion) {
    use oxi_tui::renderer::RenderToSurface;

    let mut group = c.benchmark_group("cell_to_ansi");

    let plain = Cell::new('A');
    let styled = CellBuilder::new('X')
        .foreground(Color::Rgb(255, 100, 50))
        .background(Color::Indexed(234))
        .bold()
        .italic()
        .build();

    group.bench_function("plain_cell", |b| {
        b.iter(|| black_box(plain.to_ansi()));
    });

    group.bench_function("styled_cell", |b| {
        b.iter(|| black_box(styled.to_ansi()));
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_surface_creation,
    bench_surface_clear,
    bench_surface_write_string,
    bench_surface_fill,
    bench_surface_diff,
    bench_render_full,
    bench_render_uniform,
    bench_cell_to_ansi,
);
criterion_main!(benches);
