use crate::process::{Process, ProcessId, allocate_pid};
use crate::drivers::display::display;
use crate::graphics::color::Color;
use crate::mm::memory;
use crate::println;

pub struct ShellProcess {
    id: ProcessId,
    name: &'static str,
}

impl ShellProcess {
    pub fn new() -> Self {
        Self {
            id: allocate_pid(),
            name: "shell",
        }
    }
}

impl Process for ShellProcess {
    fn get_id(&self) -> ProcessId {
        self.id
    }
    
    fn get_name(&self) -> &str {
        self.name
    }
    
    fn run(&mut self) {
        let stats = memory::get_memory_stats();
        let buffer_type = if display::USE_DOUBLE_BUFFER { " (Double Buffered)" } else { "" };
        
        println!("Welcome to AgenticOS!{}", buffer_type);
        println!("======================");
        println!();
        
        // Print memory information
        println!("Memory Statistics:");
        println!("  Total usable memory: {} MB", stats.usable_memory / (1024 * 1024));
        println!("  Total memory: {} MB", stats.total_memory / (1024 * 1024));
        println!();
        
        // Demonstrate color support
        display::set_color(Color::CYAN);
        println!("This text is in cyan!");
        
        display::set_color(Color::GREEN);
        println!("This text is in green!");
        
        display::set_color(Color::YELLOW);
        println!("This text is in yellow!");
        
        display::set_color(Color::WHITE);
        println!();
        
        // Demonstrate scrolling
        println!("Testing scrolling functionality:");
        println!("================================");
        
        for i in 0..300 {
            display::set_color(if i % 2 == 0 { Color::WHITE } else { Color::GRAY });
            println!("Line {}: This is a test of the scrolling text buffer", i + 1);
        }
        
        display::set_color(Color::MAGENTA);
        println!();
        println!("Scrolling test complete!");
        
        // Demonstrate tab support
        display::set_color(Color::WHITE);
        println!();
        println!("Tilde Test: ~");
        println!("Tab test:");
        println!("Column:\t1\t2\t3\t4");
        println!("Value:\tA\tB\tC\tD");
        
        // Final message
        println!();
        display::set_color(Color::CYAN);
        println!("AgenticOS kernel initialized successfully!");
        display::set_color(Color::WHITE);
        println!("System ready.");
    }
}