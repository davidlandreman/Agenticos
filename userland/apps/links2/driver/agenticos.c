/* AgenticOS graphics driver for Links 2.
 *
 * Links' graphics ABI is C, so this file only adapts its callbacks. Window
 * ownership, rasterization, GUI syscalls, and input translation live in the
 * no_std Rust static library under driver-rs/.
 */
#include "cfg.h"

#ifdef GRDRV_AGENTICOS

#include "links.h"
#include <stdint.h>
#include <string.h>
#include <unistd.h>

#define AG_O_NONBLOCK 0x800
#define AG_O_CLOEXEC  0x80000

struct ag_surface {
	uint32_t handle;
	uint32_t width;
	uint32_t height;
	size_t stride;
	uint32_t *pixels;
};

struct ag_gui_event {
	uint32_t kind;
	uint32_t window;
	uint32_t payload[6];
};

struct ag_mapped_event {
	uint32_t kind;
	int key, flags, x, y, buttons;
	uint32_t width, height;
};

extern struct ag_surface *ag_surface_create(uint32_t, uint32_t, const unsigned char *, size_t);
extern void ag_surface_destroy(struct ag_surface *);
extern int ag_surface_resize(struct ag_surface *, uint32_t, uint32_t);
extern long ag_surface_present(struct ag_surface *);
extern long ag_surface_set_title(struct ag_surface *, const unsigned char *, size_t);
extern long ag_event_open(uint64_t);
extern long ag_event_read(int, struct ag_gui_event *, size_t);
extern uint32_t *ag_bitmap_alloc(int, int);
extern void ag_bitmap_free(uint32_t *);
extern void ag_fill(struct ag_surface *, int, int, int, int, uint32_t, int, int, int, int);
extern void ag_draw_bitmap(struct ag_surface *, const uint32_t *, int, int, int, int, int, int, int, int);
extern void ag_scroll(struct ag_surface *, int, int, int, int, int, int);
extern void ag_map_event(const struct ag_gui_event *, struct ag_mapped_event *, uint32_t *);

extern struct graphics_driver agenticos_driver;

static int event_fd = -1;
static struct graphics_device *active_device;
static uint32_t previous_buttons;

/* EV_INIT can paint without a following driver flush. Schedule from every
 * mutating callback; Links coalesces duplicate function/data bottom halves. */
static void agenticos_present(void *data)
{
	struct graphics_device *dev = (struct graphics_device *)data;
	if (dev == active_device && dev->driver_data)
		(void)ag_surface_present((struct ag_surface *)dev->driver_data);
}

static void agenticos_schedule_present(struct graphics_device *dev)
{
	register_bottom_half(agenticos_present, dev);
}

static void agenticos_process_events(void *unused)
{
	struct ag_gui_event events[8];
	long bytes;
	int i, count;
	(void)unused;
	while ((bytes = ag_event_read(event_fd, events, 8)) > 0) {
		count = (int)(bytes / (long)sizeof(events[0]));
		for (i = 0; i < count; i++) {
			struct ag_mapped_event mapped;
			struct graphics_device *dev = active_device;
			if (!dev) continue;
			ag_map_event(&events[i], &mapped, &previous_buttons);
			switch (mapped.kind) {
			case 1:
				if (dev->keyboard_handler) dev->keyboard_handler(dev, mapped.key, mapped.flags);
				break;
			case 2:
				if (dev->mouse_handler) dev->mouse_handler(dev, mapped.x, mapped.y, mapped.buttons);
				break;
			case 3:
				if (mapped.width && mapped.height &&
				    !ag_surface_resize((struct ag_surface *)dev->driver_data, mapped.width, mapped.height)) {
					dev->size.x2 = dev->clip.x2 = (int)mapped.width;
					dev->size.y2 = dev->clip.y2 = (int)mapped.height;
					if (dev->resize_handler) dev->resize_handler(dev);
					agenticos_schedule_present(dev);
				}
				break;
			case 4:
				if (dev->keyboard_handler) dev->keyboard_handler(dev, KBD_CLOSE, 0);
				break;
			case 5:
				if (dev->redraw_handler) dev->redraw_handler(dev, &dev->size);
				break;
			}
		}
	}
}

static unsigned char *agenticos_init_driver(unsigned char *param, unsigned char *display)
{
	(void)param;
	(void)display;
	event_fd = (int)ag_event_open(AG_O_NONBLOCK | AG_O_CLOEXEC);
	if (event_fd < 0) return stracpy(cast_uchar "AgenticOS GUI event descriptor unavailable");
	set_handlers(event_fd, agenticos_process_events, NULL, NULL);
	agenticos_driver.depth = 196; /* 4-byte, 24-bit, little-endian XRGB */
	return NULL;
}

static struct graphics_device *agenticos_init_device(void)
{
	static const unsigned char title[] = "Links Web Browser";
	struct graphics_device *dev;
	struct ag_surface *surface;
	if (active_device) return NULL;
	surface = ag_surface_create(1024, 700, title, sizeof(title) - 1);
	if (!surface) return NULL;
	dev = (struct graphics_device *)mem_calloc(sizeof(*dev));
	dev->size.x1 = dev->clip.x1 = 0;
	dev->size.y1 = dev->clip.y1 = 0;
	dev->size.x2 = dev->clip.x2 = (int)surface->width;
	dev->size.y2 = dev->clip.y2 = (int)surface->height;
	dev->driver_data = surface;
	active_device = dev;
	agenticos_schedule_present(dev);
	return dev;
}

static void agenticos_shutdown_device(struct graphics_device *dev)
{
	if (!dev) return;
	unregister_bottom_half(agenticos_present, dev);
	if (active_device == dev) active_device = NULL;
	ag_surface_destroy((struct ag_surface *)dev->driver_data);
	mem_free(dev);
}

static void agenticos_shutdown_driver(void)
{
	if (event_fd >= 0) {
		set_handlers(event_fd, NULL, NULL, NULL);
		close(event_fd);
		event_fd = -1;
	}
}

static int agenticos_get_empty_bitmap(struct bitmap *bmp)
{
	bmp->data = ag_bitmap_alloc(bmp->x, bmp->y);
	if (!bmp->data) return -1;
	bmp->skip = (ssize_t)bmp->x * 4;
	bmp->flags = bmp->data;
	return 0;
}

static void agenticos_register_bitmap(struct bitmap *bmp) { (void)bmp; }
static void *agenticos_prepare_strip(struct bitmap *bmp, int top, int lines)
{
	(void)lines;
	return (unsigned char *)bmp->data + (ssize_t)top * bmp->skip;
}
static void agenticos_commit_strip(struct bitmap *bmp, int top, int lines)
{ (void)bmp; (void)top; (void)lines; }
static void agenticos_unregister_bitmap(struct bitmap *bmp)
{
	ag_bitmap_free((uint32_t *)bmp->flags);
	bmp->data = bmp->flags = NULL;
}

static void agenticos_draw_bitmap(struct graphics_device *dev, struct bitmap *bmp, int x, int y)
{
	ag_draw_bitmap((struct ag_surface *)dev->driver_data, (const uint32_t *)bmp->data,
		bmp->x, bmp->y, x, y, dev->clip.x1, dev->clip.y1, dev->clip.x2, dev->clip.y2);
	agenticos_schedule_present(dev);
}

static long agenticos_get_color(int rgb) { return rgb; }
static void agenticos_fill_area(struct graphics_device *dev, int x1, int y1, int x2, int y2, long color)
{
	ag_fill((struct ag_surface *)dev->driver_data, x1, y1, x2, y2, (uint32_t)color,
		dev->clip.x1, dev->clip.y1, dev->clip.x2, dev->clip.y2);
	agenticos_schedule_present(dev);
}
static void agenticos_draw_hline(struct graphics_device *dev, int x1, int y, int x2, long color)
{ agenticos_fill_area(dev, x1, y, x2, y + 1, color); }
static void agenticos_draw_vline(struct graphics_device *dev, int x, int y1, int y2, long color)
{ agenticos_fill_area(dev, x, y1, x + 1, y2, color); }
static int agenticos_scroll(struct graphics_device *dev, struct rect_set **set, int x, int y)
{
	(void)set;
	ag_scroll((struct ag_surface *)dev->driver_data, x, y,
		dev->clip.x1, dev->clip.y1, dev->clip.x2, dev->clip.y2);
	agenticos_schedule_present(dev);
	return 1;
}
static void agenticos_flush(struct graphics_device *dev)
{
	unregister_bottom_half(agenticos_present, dev);
	agenticos_present(dev);
}
static void agenticos_set_title(struct graphics_device *dev, unsigned char *title)
{ if (title) (void)ag_surface_set_title((struct ag_surface *)dev->driver_data, title, strlen((char *)title)); }

struct graphics_driver agenticos_driver = {
	cast_uchar "agenticos",
	agenticos_init_driver, agenticos_init_device, agenticos_shutdown_device,
	agenticos_shutdown_driver, NULL, NULL, NULL, NULL, NULL, NULL,
	agenticos_get_empty_bitmap, agenticos_register_bitmap, agenticos_prepare_strip,
	agenticos_commit_strip, agenticos_unregister_bitmap, agenticos_draw_bitmap,
	agenticos_get_color, agenticos_fill_area, agenticos_draw_hline, agenticos_draw_vline,
	agenticos_scroll, NULL, agenticos_flush, NULL, NULL, NULL, NULL,
	agenticos_set_title, NULL, NULL, NULL,
	196, 0, 0,
	GD_UNICODE_KEYS | GD_ONLY_1_WINDOW | GD_NO_OS_SHELL | GD_NO_LIBEVENT,
	NULL
};

#endif
