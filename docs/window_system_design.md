# Window System Design Document

## Executive Summary

This document outlines the design for AgenticOS's new window-based graphics system. The system will provide a unified abstraction for both GUI and text-based interfaces through a hierarchical window model, supporting multiple virtual screens, event routing, and flexible rendering capabilities. A key differentiator is the sophisticated text mode that seamlessly embeds real graphics within primarily text-based interfaces.

## Architecture Overview

The window system is built around these core concepts:

1. **Window**: The fundamental rendering and event-handling unit
2. **Screen**: A full-screen root window that can be switched between
3. **Graphics Device**: Low-level drawing interface abstracted behind a trait
4. **Window Manager**: Central coordinator for screens, focus, and input routing
5. **Event System**: Hierarchical event propagation through the window tree

## Core Abstractions

### Window

The `Window` is the base primitive for all visual elements in the system.

```rust
pub trait Window {
    // Core properties
    fn id(&self) -> WindowId;
    fn bounds(&self) -> Rect;  // Position relative to parent
    fn visible(&self) -> bool;
    
    // Hierarchy
    fn parent(&self) -> Option<WindowId>;
    fn children(&self) -> &[WindowId];
    
    // Drawing
    fn paint(&mut self, device: &mut dyn GraphicsDevice);
    fn needs_repaint(&self) -> bool;
    fn invalidate(&mut self);  // Mark for repaint
    
    // Events
    fn handle_event(&mut self, event: Event) -> EventResult;
    
    // Focus
    fn can_focus(&self) -> bool;
    fn has_focus(&self) -> bool;
}
```

### Screen

A `Screen` is a special full-screen root window that represents a virtual display.

```rust
pub struct Screen {
    id: ScreenId,
    root_window: Box<dyn Window>,
    mode: ScreenMode,  // Text or GUI
}

pub enum ScreenMode {
    Text,     // Console/terminal mode
    Gui,      // Graphical mode
}
```

### Graphics Device

The `GraphicsDevice` trait abstracts the underlying drawing operations. **Important**: There is only ONE physical framebuffer (display hardware) provided by the bootloader. All GraphicsDevice implementations ultimately write to this single framebuffer.

```rust
pub trait GraphicsDevice {
    // Device properties
    fn width(&self) -> usize;
    fn height(&self) -> usize;
    fn color_depth(&self) -> ColorDepth;
    
    // Basic drawing
    fn clear(&mut self, color: Color);
    fn draw_pixel(&mut self, x: usize, y: usize, color: Color);
    fn draw_line(&mut self, x1: usize, y1: usize, x2: usize, y2: usize, color: Color);
    fn draw_rect(&mut self, x: usize, y: usize, width: usize, height: usize, color: Color);
    fn fill_rect(&mut self, x: usize, y: usize, width: usize, height: usize, color: Color);
    
    // Text drawing
    fn draw_text(&mut self, x: usize, y: usize, text: &str, font: &Font, color: Color);
    
    // Image drawing
    fn draw_image(&mut self, x: usize, y: usize, image: &Image);
    
    // Clipping
    fn set_clip_rect(&mut self, rect: Option<Rect>);
    
    // Buffer management
    fn flush(&mut self);  // For double-buffered implementations
}
```

### Window Manager

The `WindowManager` is a single global instance that coordinates all windows across all screens. It is initialized during kernel boot and manages the entire display system, including mouse cursor rendering.

```rust
// Global window manager instance
static WINDOW_MANAGER: Mutex<Option<WindowManager>> = Mutex::new(None);

pub struct WindowManager {
    screens: Vec<Screen>,
    active_screen: ScreenId,
    window_registry: BTreeMap<WindowId, Box<dyn Window>>,
    focus_stack: Vec<WindowId>,  // Top is focused
    z_order: Vec<WindowId>,      // Back to front
    graphics_device: Box<dyn GraphicsDevice>,
    mouse_cursor: MouseCursor,    // Owns mouse rendering
    last_mouse_pos: (usize, usize),
}

impl WindowManager {
    // Screen management
    pub fn create_screen(&mut self, mode: ScreenMode) -> ScreenId;
    pub fn switch_screen(&mut self, screen: ScreenId);
    pub fn get_active_screen(&self) -> &Screen;
    
    // Window management
    pub fn create_window(&mut self, parent: Option<WindowId>) -> WindowId;
    pub fn destroy_window(&mut self, id: WindowId);
    pub fn move_window(&mut self, id: WindowId, x: i32, y: i32);
    pub fn resize_window(&mut self, id: WindowId, width: u32, height: u32);
    
    // Focus management
    pub fn focus_window(&mut self, id: WindowId);
    pub fn focused_window(&self) -> Option<WindowId>;
    
    // Event routing
    pub fn route_keyboard_event(&mut self, event: KeyboardEvent);
    pub fn route_mouse_event(&mut self, event: MouseEvent);
    
    // Rendering
    pub fn render(&mut self, device: &mut dyn GraphicsDevice);
}
```

## Window Hierarchy and Types

### Base Window Types

1. **ContainerWindow**: Can hold child windows
2. **TextWindow**: Grid-based text rendering
3. **CanvasWindow**: Free-form graphics drawing
4. **FrameWindow**: Window with decorations (title bar, borders)

### Control Windows

All UI controls are specialized windows:

- **Button**: Clickable control with label
- **TextBox**: Text input field
- **Label**: Static text display
- **ListView**: Scrollable list of items
- **Menu**: Dropdown or popup menu

### Example Hierarchy

```
Screen (GUI Mode)
├── FrameWindow "Terminal"
│   ├── TitleBar
│   ├── MenuBar
│   └── TextWindow (terminal content)
│       └── InlineImage (for terminal graphics)
└── FrameWindow "Editor"
    ├── TitleBar
    ├── TextBox (editor content)
    └── StatusBar
```

## Event System

### Event Types

```rust
pub enum Event {
    // Input events
    Keyboard(KeyboardEvent),
    Mouse(MouseEvent),
    
    // Window events
    Resize(ResizeEvent),
    Move(MoveEvent),
    Close(CloseEvent),
    Focus(FocusEvent),
    
    // Custom events
    Custom(Box<dyn Any>),
}

pub enum EventResult {
    Handled,      // Event was processed
    Ignored,      // Event not relevant
    Propagate,    // Pass to parent
}
```

### Event Flow

1. Window Manager receives raw input
2. Routes to focused window (keyboard) or window under cursor (mouse)
3. Window processes event and returns result
4. If `Propagate`, event bubbles up to parent
5. Continue until handled or reaching root

## Rendering Pipeline

### Double Buffering Per Window

Each window maintains its own buffer to enable:
- Efficient partial redraws
- Window compositing
- Future transparency/effects

```rust
pub struct WindowBuffer {
    pixels: Vec<u32>,  // RGBA buffer
    width: usize,
    height: usize,
    dirty_region: Option<Rect>,  // Area needing redraw
}
```

### Rendering Process

1. **Invalidation**: Window marks itself dirty via `invalidate()`
2. **Traversal**: Window manager walks window tree during render
3. **Clipping**: Set clip rectangle to window bounds
4. **Drawing**: Window paints itself via `paint()` method
5. **Recursion**: Window asks children to paint
6. **Compositing**: Buffers combined respecting z-order
7. **Present**: Final composed image sent to THE physical framebuffer

#### Hardware Reality

Only one component can write to the physical framebuffer at a time:
- Window Manager owns the GraphicsDevice
- GraphicsDevice owns the physical framebuffer reference
- All screen switches are just changing what gets rendered
- The physical display hardware remains constant

```rust
impl WindowManager {
    pub fn render(&mut self) {
        // Compose all windows for active screen into device buffer
        self.render_active_screen();
        
        // Draw mouse cursor as final overlay
        self.render_mouse_cursor();
        
        // This is the ONLY place that touches physical hardware
        self.graphics_device.flush(); // Swaps to physical framebuffer
    }
    
    fn render_mouse_cursor(&mut self) {
        let (mouse_x, mouse_y) = mouse::get_position();
        // Save background, draw cursor at position
        self.graphics_device.draw_cursor(mouse_x, mouse_y);
    }
}
```

### Text Mode Rendering

Text mode in AgenticOS is a sophisticated text-first hybrid mode that differentiates this OS from traditional terminals:
- Root screen is a full-screen TextWindow
- **Real graphics windows can be embedded within text** - not ASCII art representations
- Images display as actual rendered graphics inline with text
- UI controls render with full graphical fidelity when needed
- Primary interface remains text-based, but graphics enhance functionality where appropriate

This allows for innovative interfaces like:
- Terminal commands that output inline charts and visualizations
- Text editors with embedded image previews
- Command-line tools with graphical buttons for common actions
- System monitors showing real-time graphs alongside text data

## Integration with Existing Systems

### Graphics Subsystem Refactoring

The existing graphics modules will be reorganized into the new architecture:

#### Physical Framebuffer Constraint

**Critical Understanding**: The bootloader provides exactly ONE physical framebuffer that represents the actual display hardware. This is a fixed memory region that, when written to, appears on screen. All our abstractions must share this single resource.

```rust
// There's only ONE of these in the entire system!
static mut PHYSICAL_FRAMEBUFFER: Option<&'static mut FrameBuffer> = None;
```

#### Current Module Mapping

1. **`frame_buffer.rs`** → `DirectFrameBufferDevice`
   - Wraps the single physical framebuffer from bootloader
   - Direct writes to display hardware, no buffering
   - When active, has exclusive access to the framebuffer
   - Useful for early boot or debugging

2. **`double_buffer.rs` + `double_buffered_text.rs`** → `DoubleBufferedDevice`
   - Also wraps the same physical framebuffer
   - Adds an 8MB static back buffer for performance
   - Only the "swap_buffers" operation touches the physical framebuffer
   - When active, has exclusive access to the framebuffer

3. **`core_gfx.rs`** → Graphics methods in `GraphicsDevice` trait
   - All primitive drawing methods (lines, circles, etc.) become trait methods
   - Each device implementation provides these primitives
   - No longer a separate struct, functionality is part of the device

4. **`core_text.rs`** → Text rendering in Windows
   - `TextRenderer` functionality moves into `TextWindow` implementation
   - Font management remains separate but is used by windows
   - Each window handles its own text layout

#### Initialization Flow

```rust
// In kernel.rs during boot
pub fn kernel_main() {
    // ... early initialization ...
    
    // Create graphics device with framebuffer
    let device = DoubleBufferedDevice::new(framebuffer);
    
    // Initialize global window manager
    window::init_window_manager(device);
    
    // Create initial GUI screen with terminal
    let gui_screen = window::create_screen(ScreenMode::Gui);
    let terminal_window = window::create_terminal_window();
    
    // Shell runs inside the terminal window
    let shell = ShellProcess::new();
    terminal_window.attach_process(shell);
    
    // ... rest of initialization ...
}
```

### Process Manager

Input routing needs coordination:
- Window Manager owns keyboard/mouse event routing
- Focused window's process receives stdin
- ProcessManager notifies Window Manager of focus requests

### Shell Integration

The shell process runs inside a terminal window, not as a window itself:

#### Terminal Window
- `TerminalWindow` is a specialized `TextWindow` with terminal emulation
- Provides stdin/stdout/stderr streams to attached processes
- Handles ANSI escape sequences, cursor control, scrollback
- Can embed graphics windows for inline images/charts

#### Shell Process Architecture
```rust
// Shell doesn't know about windows
pub struct ShellProcess {
    stdin: Arc<Mutex<StdinBuffer>>,
    stdout: Arc<Mutex<TerminalOutput>>,
    // ... shell state ...
}

// Terminal window manages the shell
pub struct TerminalWindow {
    window_base: WindowBase,
    shell_process: Option<ShellProcess>,
    text_buffer: TextBuffer,
    cursor_pos: (usize, usize),
}
```

#### Boot Modes

1. **GUI Mode (default)**:
   - Window Manager creates GUI screen
   - Creates `FrameWindow` with decorations
   - Embeds `TerminalWindow` inside frame
   - Shell process attached to terminal

2. **Text Mode**:
   - Window Manager creates Text screen
   - Full-screen `TerminalWindow` (no decorations)
   - Shell process attached to terminal
   - Same shell code runs in both modes

## Implementation Status

### Phase 1: Core Infrastructure ✅ COMPLETED
- [x] Window trait and basic types
- [x] GraphicsDevice trait
- [x] WindowManager skeleton
- [x] Event system

### Phase 2: Basic Windows ✅ COMPLETED
- [x] ContainerWindow
- [x] TextWindow with grid rendering
- [x] Simple event routing
- [x] Screen switching

### Phase 3: Graphics Integration ✅ COMPLETED
- [ ] CanvasWindow (not implemented)
- [x] GraphicsDevice implementation with both DirectFrameBuffer and DoubleBuffered adapters
- [x] Clipping support
- [ ] Window buffers (decided against for performance)

### Phase 4: UI Controls 🚧 PARTIAL
- [ ] FrameWindow with decorations
- [ ] Button, Label, TextBox
- [x] Focus management
- [x] Mouse interaction and cursor rendering

### Phase 5: Advanced Features ⏳ FUTURE
- [ ] Drag and drop
- [ ] Window resizing/moving
- [ ] Menus and dialogs
- [ ] Inline terminal graphics

## Migration from Current System

### What Happens to Current Code

1. **Global print macros** (`println!`, `print!`):
   - Route through the focused terminal window
   - Early boot can use a simple bootstrap terminal
   - Debug output goes to serial as before

2. **Current display initialization**:
   ```rust
   // OLD: display::init(framebuffer);
   // NEW: window::init_window_manager(device);
   ```

3. **Mouse cursor rendering**:
   - Currently in kernel idle loop - **MUST MOVE** to Window Manager
   - Window Manager will handle cursor as overlay layer  
   - Mouse driver still initialized early, but rendering owned by WM
   - No more mouse handling in kernel.rs idle loop

4. **Existing commands and processes**:
   - Continue to work unchanged
   - Now run inside terminal windows
   - Can spawn GUI windows if needed

### Backwards Compatibility

- Existing shell commands work without modification
- `println!` and text output automatically route to focused terminal
- Graphics commands can detect window mode and render appropriately

## Open Questions and Considerations

1. **Memory Management**: Should window buffers use static allocation or heap?
   - Initial implementation: Heap allocation for flexibility
   - Future optimization: Pool of pre-allocated buffers

2. **Transparency**: How to handle alpha blending in compositing?
   - Phase 1: No transparency, simple overdraw
   - Phase 2: Alpha channel support in WindowBuffer

3. **Performance**: When to use dirty rectangles vs full redraws?
   - Start with dirty rectangles from day 1
   - Profile to determine optimal threshold

4. **Text Rendering**: Should TextWindow cache rendered glyphs?
   - Yes, glyph cache per font for performance

5. **Screen Switching**: How to handle Ctrl+Alt+F1-F12 style switching?
   - Window Manager intercepts special key combos
   - Maintains screen state during switches

## Kernel Boot Sequence

```rust
// In kernel.rs
pub fn kernel_main(boot_info: &'static mut BootInfo) {
    // ... memory initialization ...
    
    // Initialize mouse driver (hardware only, no rendering)
    mouse::init();
    
    // Get framebuffer from bootloader
    let framebuffer = boot_info.framebuffer.as_mut().unwrap();
    
    // Create graphics device (replaces current display::init)
    let device = Box::new(DoubleBufferedDevice::new(framebuffer));
    
    // Initialize global Window Manager (now owns mouse rendering)
    window::init_window_manager(device);
    
    // Create default GUI screen with terminal
    window::create_default_desktop();
    
    // The shell is now running in a terminal window
    // All println! macros route through the focused terminal
}

pub fn kernel_idle() -> ! {
    loop {
        // Window Manager handles all rendering including mouse
        window::render_frame();
        
        // No more mouse handling here!
        x86_64::instructions::hlt();
    }
}

// In window/mod.rs
pub fn create_default_desktop() {
    with_window_manager(|wm| {
        // Create GUI screen
        let screen_id = wm.create_screen(ScreenMode::Gui);
        wm.switch_screen(screen_id);
        
        // Create main terminal window with frame
        let frame = wm.create_window(None);
        wm.set_window_impl(frame, Box::new(FrameWindow::new("AgenticOS Terminal")));
        wm.move_window(frame, 50, 50);
        wm.resize_window(frame, 800, 600);
        
        // Create terminal inside frame
        let terminal = wm.create_window(Some(frame));
        let mut term_window = TerminalWindow::new();
        
        // Attach shell process to terminal
        let shell = ShellProcess::new();
        term_window.attach_process(shell);
        
        wm.set_window_impl(terminal, Box::new(term_window));
        wm.focus_window(terminal);
    });
}
```

## Example Application Usage

```rust
// Creating a custom application window
pub fn create_text_editor() {
    window::with_window_manager(|wm| {
        // Create framed window
        let frame = wm.create_window(None);
        wm.set_window_impl(frame, Box::new(FrameWindow::new("Text Editor")));
        
        // Add menu bar
        let menu_bar = wm.create_window(Some(frame));
        wm.set_window_impl(menu_bar, Box::new(MenuBar::new()));
        
        // Add text editing area
        let editor = wm.create_window(Some(frame));
        wm.set_window_impl(editor, Box::new(TextBox::multiline()));
        
        // Layout children
        frame.layout_children();
    });
}
```

## Implementation Lessons Learned

### Performance Insights

1. **Double Buffering vs Direct Rendering**
   - Double buffering requires copying ~3.5MB on every frame for 1280x720
   - For mostly static content with just mouse movement, direct framebuffer writes are much faster
   - The window system now uses direct framebuffer mode by default (USE_DOUBLE_BUFFER = false)

2. **Smart Rendering**
   - Only render frames when something actually changes (mouse movement, new text, window invalidation)
   - Use HLT instruction to save CPU between frames
   - Track dirty state to avoid unnecessary buffer swaps

3. **Mouse Cursor Optimization**
   - Originally tried to implement save/restore for cursor background
   - Simpler approach: use fast mouse update that only redraws cursor area
   - Future improvement: hardware cursor support if available

### Architecture Decisions

1. **Print Macro Integration**
   - Print macros check if window system is available and route through console buffer
   - TextWindow pulls from console buffer during paint()
   - This allows seamless transition from boot-time direct printing to window system

2. **Window Rendering Pipeline**
   - Windows are temporarily removed from registry during rendering to avoid borrow checker issues
   - Recursive rendering with proper clipping for child windows
   - Z-order maintained for proper layering

3. **Graphics Device Abstraction**
   - Two adapters implemented: DirectFrameBufferDevice and DoubleBufferedDevice
   - Easy to switch between them based on USE_DOUBLE_BUFFER flag
   - Consistent interface allows future optimizations (dirty rectangles, hardware acceleration)

### Current Status

The window system is functional with:
- ✅ Hierarchical window management
- ✅ Mouse cursor with good performance
- ✅ Text output through windows
- ✅ Event system foundation
- ✅ Multiple screen support

Not yet implemented:
- ❌ Interactive shell (needs async keyboard input)
- ❌ Window decorations and controls
- ❌ Drag and drop
- ❌ Proper save/restore for overlapping windows

## Conclusion

This window system design provides a flexible foundation for both text and graphical interfaces in AgenticOS. By treating everything as a window with a common event and rendering model, we can build sophisticated user interfaces while maintaining simplicity and consistency. The unique text-first hybrid mode sets AgenticOS apart by allowing rich graphical content to enhance text-based workflows without sacrificing the efficiency and clarity of a terminal interface.

The implementation revealed important performance considerations around framebuffer access patterns and the trade-offs between double buffering and direct rendering. The current system achieves good mouse responsiveness while maintaining a clean architecture for future enhancements. 