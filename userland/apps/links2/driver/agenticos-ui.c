/* Semantic Links menu adapter for the AgenticOS native widget host. */
#include "cfg.h"

#ifdef GRDRV_AGENTICOS

#include "links.h"
#include "agenticos_ui.h"
#include <stdint.h>
#include <string.h>

extern struct graphics_driver agenticos_driver;
extern long ag_surface_present(struct ag_surface *);

struct agenticos_menu_registry {
	uint64_t id;
	struct terminal *term;
	struct menu_item *items;
	void *data;
	int count;
	int owns_menu_items;
	void (*free_function)(void *);
	void *free_data;
};

static struct agenticos_menu_registry active_menu;
static struct ag_ui_result pending_result;
static uint64_t next_menu_id = 1;
static int dispatch_queued;
static struct session *bound_session;

struct agenticos_dialog_label {
	unsigned char *text;
	int x, y;
};

struct agenticos_dialog_row {
	unsigned char *text;
	int x, y, width, height;
	int depth, type, selected;
};

struct agenticos_dialog_registry {
	uint64_t id;
	struct dialog_data *dialog;
	int label_count;
	int row_count;
	int capturing;
	int has_progress;
	int progress_x, progress_y, progress_w;
	int progress_value, progress_max;
	int has_scrollbar;
	int scrollbar_x, scrollbar_y, scrollbar_w, scrollbar_h;
	int scrollbar_total, scrollbar_view, scrollbar_pos;
	struct agenticos_dialog_label labels[256];
	struct agenticos_dialog_row rows[192];
};

static struct agenticos_dialog_registry active_dialog;

static void clear_dialog_labels(void)
{
	int i;
	for (i = 0; i < active_dialog.label_count; i++)
		mem_free(active_dialog.labels[i].text);
	for (i = 0; i < active_dialog.row_count; i++)
		mem_free(active_dialog.rows[i].text);
	active_dialog.label_count = 0;
	active_dialog.row_count = 0;
	active_dialog.has_progress = 0;
	active_dialog.has_scrollbar = 0;
}

static void release_menu_registry(void)
{
	int i;
	struct agenticos_menu_registry old = active_menu;
	memset(&active_menu, 0, sizeof(active_menu));
	if (!old.id) return;
	if (old.owns_menu_items && old.items->free_i) {
		for (i = 0; i < old.count; i++) {
			if (old.items[i].free_i & MENU_FREE_TEXT) mem_free(old.items[i].text);
			if (old.items[i].free_i & MENU_FREE_RTEXT) mem_free(old.items[i].rtext);
			if (old.items[i].free_i & MENU_FREE_HOTKEY) mem_free(old.items[i].hotkey);
		}
		if (old.items->free_i & MENU_FREE_ITEMS) mem_free(old.items);
	}
	if (old.free_function)
		register_bottom_half(old.free_function, old.free_data);
}

static void close_active_menu(void)
{
	struct ag_surface *surface;
	if (!active_menu.id) return;
	surface = (struct ag_surface *)active_menu.term->dev->driver_data;
	ag_ui_close(surface, active_menu.id);
	release_menu_registry();
	(void)ag_surface_present(surface);
}

static void dispatch_native_action(void *unused)
{
	struct ag_ui_result action = pending_result;
	struct menu_item *item;
	void (*function)(struct terminal *, void *, void *);
	struct terminal *term;
	void *item_data, *menu_data;
	uint64_t generation;
	int index;
	(void)unused;
	dispatch_queued = 0;
	if (action.action_kind == AG_UI_ACTION_CHROME) {
		if (!bound_session) return;
		if (action.value >= 100 && action.value < 106)
			activate_bfu_technology(bound_session, (int)action.value - 100);
		else if (action.value == 1)
			go_back(bound_session, 1);
		else if (action.value == 2)
			go_back(bound_session, -1);
		else if (action.value == 3)
			reload(bound_session, -1);
		else if (action.value == 4)
			dialog_goto_url(bound_session, NULL);
		return;
	}
	if (!active_menu.id || action.target != active_menu.id) return;
	if (action.action_kind == AG_UI_ACTION_CANCEL) {
		close_active_menu();
		return;
	}
	if (action.action_kind != AG_UI_ACTION_ACTIVATE) return;
	index = (int)action.value;
	if (index < 0 || index >= active_menu.count) return;
	item = &active_menu.items[index];
	if (item->hotkey == M_BAR || !item->func) return;
	function = item->func;
	item_data = item->data;
	menu_data = active_menu.data;
	term = active_menu.term;
	generation = active_menu.id;
	if (!item->in_m) close_active_menu();
	function(term, item_data, menu_data);
	/* An in-menu callback normally opens a child menu. If it did not, keep
	 * the current surface available just as Links keeps its menu window. */
	if (item->in_m && active_menu.id == generation)
		(void)ag_surface_present((struct ag_surface *)term->dev->driver_data);
}

void agenticos_ui_accept_result(const struct ag_ui_result *result)
{
	if (!result || result->version != AG_UI_ABI_VERSION ||
	    result->byte_len != sizeof(*result) || !result->action_kind || dispatch_queued)
		return;
	pending_result = *result;
	dispatch_queued = 1;
	register_bottom_half(dispatch_native_action, NULL);
}

int agenticos_ui_open_links_menu(struct terminal *term, struct menu_item *items,
	void *data, int selected, void (*free_function)(void *), void *free_data,
	int main_menu)
{
	struct ag_ui_node *nodes;
	struct ag_surface *surface;
	uint64_t id;
	size_t total_text = 0;
	int i, result;
	if (!F || !term || !term->dev || drv != &agenticos_driver) return 0;
	for (i = 0; items[i].text; i++) {
		unsigned char *label, *secondary;
		if (i >= (int)AG_UI_MAX_MENU_NODES) return 0;
		label = get_text_translation(items[i].text, term);
		secondary = get_text_translation(items[i].rtext, term);
		total_text += strlen((char *)label) + strlen((char *)secondary);
		if (total_text > AG_UI_MAX_TEXT_BYTES) return 0;
	}
	if (!i) return 0;
	if (main_menu && selected >= 0 && selected < i) {
		items[selected].func(term, items[selected].data, data);
		return 1;
	}
	nodes = mem_calloc((size_t)i * sizeof(*nodes));
	for (result = 0; result < i; result++) {
		unsigned char *label = get_text_translation(items[result].text, term);
		unsigned char *secondary = get_text_translation(items[result].rtext, term);
		nodes[result].version = AG_UI_ABI_VERSION;
		nodes[result].byte_len = sizeof(nodes[result]);
		nodes[result].id = (uint64_t)result;
		nodes[result].label.ptr = label;
		nodes[result].label.len = (uint32_t)strlen((char *)label);
		if (items[result].hotkey == M_BAR && !label[0])
			nodes[result].flags |= AG_UI_NODE_SEPARATOR;
		if (!strcmp((char *)secondary, ">")) {
			nodes[result].flags |= AG_UI_NODE_SUBMENU;
			secondary = cast_uchar "";
		}
		nodes[result].secondary.ptr = secondary;
		nodes[result].secondary.len = (uint32_t)strlen((char *)secondary);
	}
	surface = (struct ag_surface *)term->dev->driver_data;
	id = next_menu_id++;
	if (!id) id = next_menu_id++;
	result = ag_ui_menu_open(surface, id, nodes, (size_t)i, selected, 4, 26);
	mem_free(nodes);
	if (result) return 0;
	release_menu_registry();
	active_menu.id = id;
	active_menu.term = term;
	active_menu.items = items;
	active_menu.data = data;
	active_menu.count = i;
	active_menu.owns_menu_items = !main_menu;
	active_menu.free_function = free_function;
	active_menu.free_data = free_data;
	(void)ag_surface_present(surface);
	return 1;
}

void agenticos_ui_shutdown(struct ag_surface *surface)
{
	if (active_menu.id) {
		ag_ui_close(surface, active_menu.id);
		release_menu_registry();
	}
	if (active_dialog.id) {
		ag_ui_dialog_close(surface, active_dialog.id);
		clear_dialog_labels();
		memset(&active_dialog, 0, sizeof(active_dialog));
	}
	dispatch_queued = 0;
	bound_session = NULL;
}

int agenticos_ui_set_document_view(void *session, int *x, int *y, int *width, int *height)
{
	struct session *ses = (struct session *)session;
	if (!ses || !ses->term || !ses->term->dev || drv != &agenticos_driver) return 0;
	bound_session = ses;
	*x = 0;
	*y = 64;
	*width = ses->term->x;
	*height = ses->term->y > 88 ? ses->term->y - 88 : 0;
	ag_ui_chrome_show((struct ag_surface *)ses->term->dev->driver_data, 1);
	return 1;
}

int agenticos_ui_set_status(struct terminal *term, const unsigned char *status)
{
	struct ag_surface *surface;
	if (!F || !term || !term->dev || drv != &agenticos_driver) return 0;
	surface = (struct ag_surface *)term->dev->driver_data;
	if (!status) status = cast_uchar "";
	ag_ui_chrome_set_status(surface, status, strlen((const char *)status));
	return 1;
}

int agenticos_ui_set_location(struct terminal *term, const unsigned char *location)
{
	struct ag_surface *surface;
	if (!F || !term || !term->dev || drv != &agenticos_driver) return 0;
	surface = (struct ag_surface *)term->dev->driver_data;
	if (!location) location = cast_uchar "";
	ag_ui_chrome_set_location(surface, location, strlen((const char *)location));
	return 1;
}

int agenticos_ui_main_menu_key(struct terminal *term, int key, int flags)
{
	int i;
	(void)flags;
	if (!active_menu.id || active_menu.term != term || key <= ' ') return 0;
	for (i = 0; i < active_menu.count; i++) {
		unsigned char *hotkey = get_text_translation(active_menu.items[i].hotkey, term);
		if (hotkey && hotkey[0] && upcase(hotkey[0]) == upcase(key)) {
			active_menu.items[i].func(term, active_menu.items[i].data, active_menu.data);
			return 1;
		}
	}
	return 0;
}

void agenticos_ui_dialog_begin(struct dialog_data *dlg)
{
	struct ag_surface *surface;
	if (!dlg || !dlg->win || !dlg->win->term || !dlg->win->term->dev ||
	    drv != &agenticos_driver) return;
	if (active_dialog.id && active_dialog.dialog != dlg) {
		surface = (struct ag_surface *)active_dialog.dialog->win->term->dev->driver_data;
		ag_ui_dialog_close(surface, active_dialog.id);
		clear_dialog_labels();
		memset(&active_dialog, 0, sizeof(active_dialog));
	}
	if (!active_dialog.id) {
		active_dialog.id = next_menu_id++;
		if (!active_dialog.id) active_dialog.id = next_menu_id++;
		active_dialog.dialog = dlg;
	}
	clear_dialog_labels();
	active_dialog.capturing = 1;
}

void agenticos_ui_dialog_label(struct dialog_data *dlg, const unsigned char *text, int x, int y)
{
	struct agenticos_dialog_label *label;
	if (active_dialog.dialog != dlg || !text || !text[0] ||
	    active_dialog.label_count >= 256) return;
	label = &active_dialog.labels[active_dialog.label_count++];
	label->text = stracpy(text);
	label->x = x;
	label->y = y;
}

void agenticos_ui_dialog_progress(struct dialog_data *dlg, int x, int y, int width,
	int value, int maximum)
{
	if (active_dialog.dialog != dlg || maximum <= 0) return;
	active_dialog.has_progress = 1;
	active_dialog.progress_x = x;
	active_dialog.progress_y = y;
	active_dialog.progress_w = width;
	active_dialog.progress_value = value;
	active_dialog.progress_max = maximum;
}

void agenticos_ui_dialog_list_row(struct dialog_data *dlg, const unsigned char *text,
	int x, int y, int width, int height, int depth, int type, int selected)
{
	struct agenticos_dialog_row *row = NULL;
	int i;
	if (active_dialog.dialog != dlg || !text || width <= 0 || height <= 0) return;
	for (i = 0; i < active_dialog.row_count; i++) {
		if (active_dialog.rows[i].x == x && active_dialog.rows[i].y == y) {
			row = &active_dialog.rows[i];
			mem_free(row->text);
			break;
		}
	}
	if (!row) {
		if (active_dialog.row_count >= 192) return;
		row = &active_dialog.rows[active_dialog.row_count++];
	}
	row->text = stracpy(text);
	row->x = x;
	row->y = y;
	row->width = width;
	row->height = height;
	row->depth = depth < 0 ? 0 : depth;
	row->type = type;
	row->selected = selected;
	if (!active_dialog.capturing) agenticos_ui_dialog_refresh(dlg);
}

void agenticos_ui_dialog_scrollbar(struct dialog_data *dlg, int x, int y,
	int width, int height, int total, int view, int pos)
{
	if (active_dialog.dialog != dlg || width <= 0 || height <= 0) return;
	active_dialog.has_scrollbar = 1;
	active_dialog.scrollbar_x = x;
	active_dialog.scrollbar_y = y;
	active_dialog.scrollbar_w = width;
	active_dialog.scrollbar_h = height;
	active_dialog.scrollbar_total = total;
	active_dialog.scrollbar_view = view;
	active_dialog.scrollbar_pos = pos;
	if (!active_dialog.capturing) agenticos_ui_dialog_refresh(dlg);
}

void agenticos_ui_dialog_refresh(struct dialog_data *dlg)
{
	struct ag_ui_node *nodes;
	struct ag_surface *surface;
	int i, count, at = 0;
	if (!active_dialog.id || active_dialog.dialog != dlg) return;
	count = 1 + active_dialog.label_count + active_dialog.row_count + dlg->n +
		active_dialog.has_progress + active_dialog.has_scrollbar;
	if (count > 256) return;
	nodes = mem_calloc((size_t)count * sizeof(*nodes));
#define INIT_NODE(node_) do { \
	(node_)->version = AG_UI_ABI_VERSION; \
	(node_)->byte_len = sizeof(*(node_)); \
} while (0)
	INIT_NODE(&nodes[at]);
	nodes[at].kind = AG_UI_NODE_DIALOG;
	nodes[at].id = active_dialog.id;
	nodes[at].x = dlg->x;
	nodes[at].y = dlg->y;
	nodes[at].width = (uint32_t)dlg->xw;
	nodes[at].height = (uint32_t)dlg->yw;
	nodes[at].label.ptr = get_text_translation(dlg->dlg->title, dlg->win->term);
	nodes[at].label.len = (uint32_t)strlen((char *)nodes[at].label.ptr);
	at++;
	for (i = 0; i < active_dialog.label_count; i++, at++) {
		INIT_NODE(&nodes[at]);
		nodes[at].kind = AG_UI_NODE_LABEL;
		nodes[at].id = (uint64_t)at;
		nodes[at].x = active_dialog.labels[i].x;
		nodes[at].y = active_dialog.labels[i].y;
		nodes[at].width = (uint32_t)(dlg->x + dlg->xw - active_dialog.labels[i].x);
		nodes[at].height = G_BFU_FONT_SIZE;
		nodes[at].label.ptr = active_dialog.labels[i].text;
		nodes[at].label.len = (uint32_t)strlen((char *)active_dialog.labels[i].text);
	}
	for (i = 0; i < active_dialog.row_count; i++, at++) {
		struct agenticos_dialog_row *row = &active_dialog.rows[i];
		INIT_NODE(&nodes[at]);
		nodes[at].kind = AG_UI_NODE_TREE_ROW;
		nodes[at].id = (uint64_t)at;
		nodes[at].x = row->x;
		nodes[at].y = row->y;
		nodes[at].width = (uint32_t)row->width;
		nodes[at].height = (uint32_t)row->height;
		nodes[at].group = (uint32_t)row->depth;
		nodes[at].value = row->type;
		if (row->selected) nodes[at].flags |= AG_UI_NODE_FOCUSED;
		nodes[at].label.ptr = row->text;
		nodes[at].label.len = (uint32_t)strlen((char *)row->text);
	}
	for (i = 0; i < dlg->n; i++, at++) {
		struct dialog_item_data *item = &dlg->items[i];
		INIT_NODE(&nodes[at]);
		nodes[at].id = (uint64_t)i;
		nodes[at].x = item->x;
		nodes[at].y = item->y;
		nodes[at].width = (uint32_t)(item->l > 0 ? item->l : 18);
		nodes[at].height = G_BFU_FONT_SIZE > 20 ? (uint32_t)G_BFU_FONT_SIZE : 20;
		if (i == dlg->selected) nodes[at].flags |= AG_UI_NODE_FOCUSED;
		switch (item->item->type) {
		case D_CHECKBOX:
			nodes[at].kind = item->item->gid ? AG_UI_NODE_RADIO : AG_UI_NODE_CHECKBOX;
			nodes[at].value = item->checked;
			break;
		case D_FIELD:
		case D_FIELD_PASS:
			nodes[at].kind = item->item->type == D_FIELD ? AG_UI_NODE_FIELD : AG_UI_NODE_PASSWORD;
			nodes[at].label.ptr = item->cdata;
			nodes[at].label.len = (uint32_t)strlen((char *)item->cdata);
			break;
		case D_BUTTON:
			nodes[at].kind = AG_UI_NODE_BUTTON;
			nodes[at].label.ptr = get_text_translation(item->item->text, dlg->win->term);
			nodes[at].label.len = (uint32_t)strlen((char *)nodes[at].label.ptr);
			break;
		}
	}
	if (active_dialog.has_progress) {
		INIT_NODE(&nodes[at]);
		nodes[at].kind = AG_UI_NODE_PROGRESS;
		nodes[at].id = (uint64_t)at;
		nodes[at].x = active_dialog.progress_x;
		nodes[at].y = active_dialog.progress_y;
		nodes[at].width = (uint32_t)active_dialog.progress_w;
		nodes[at].height = 18;
		nodes[at].value = (int64_t)active_dialog.progress_value * 1000 /
			active_dialog.progress_max;
		at++;
	}
	if (active_dialog.has_scrollbar) {
		INIT_NODE(&nodes[at]);
		nodes[at].kind = AG_UI_NODE_SCROLLBAR;
		nodes[at].flags = 1;
		nodes[at].id = (uint64_t)at;
		nodes[at].x = active_dialog.scrollbar_x;
		nodes[at].y = active_dialog.scrollbar_y;
		nodes[at].width = (uint32_t)active_dialog.scrollbar_w;
		nodes[at].height = (uint32_t)active_dialog.scrollbar_h;
		nodes[at].value = active_dialog.scrollbar_total;
		nodes[at].value_min = active_dialog.scrollbar_view;
		nodes[at].value_max = active_dialog.scrollbar_pos;
		at++;
	}
#undef INIT_NODE
	surface = (struct ag_surface *)dlg->win->term->dev->driver_data;
	(void)ag_ui_dialog_update(surface, active_dialog.id, nodes, (size_t)at, dlg->selected);
	mem_free(nodes);
	active_dialog.capturing = 0;
	(void)ag_surface_present(surface);
}

void agenticos_ui_dialog_close(struct dialog_data *dlg)
{
	struct ag_surface *surface;
	if (!active_dialog.id || active_dialog.dialog != dlg) return;
	surface = (struct ag_surface *)dlg->win->term->dev->driver_data;
	ag_ui_dialog_close(surface, active_dialog.id);
	clear_dialog_labels();
	memset(&active_dialog, 0, sizeof(active_dialog));
	(void)ag_surface_present(surface);
}

void agenticos_ui_form_control(struct terminal *term, int type, int x, int y,
	int width, int height, const unsigned char *text, int value, int focused)
{
	struct ag_ui_node node;
	if (!term || !term->dev || drv != &agenticos_driver || width <= 0 || height <= 0)
		return;
	memset(&node, 0, sizeof(node));
	node.version = AG_UI_ABI_VERSION;
	node.byte_len = sizeof(node);
	node.x = x;
	node.y = y;
	node.width = (uint32_t)width;
	node.height = (uint32_t)height;
	node.value = value;
	if (focused) node.flags |= AG_UI_NODE_FOCUSED;
	if (!text) text = cast_uchar "";
	node.label.ptr = text;
	node.label.len = (uint32_t)strlen((const char *)text);
	switch (type) {
	case FC_TEXT: case FC_FILE_UPLOAD: node.kind = AG_UI_NODE_FIELD; break;
	case FC_PASSWORD: node.kind = AG_UI_NODE_PASSWORD; break;
	case FC_TEXTAREA: node.kind = AG_UI_NODE_TEXTAREA; break;
	case FC_CHECKBOX: node.kind = AG_UI_NODE_CHECKBOX; break;
	case FC_RADIO: node.kind = AG_UI_NODE_RADIO; break;
	case FC_SELECT: node.kind = AG_UI_NODE_COMBO; break;
	case FC_SUBMIT: case FC_RESET: case FC_BUTTON: node.kind = AG_UI_NODE_BUTTON; break;
	default: return;
	}
	ag_ui_control_draw((struct ag_surface *)term->dev->driver_data, &node);
}

void agenticos_ui_scrollbar(void *device, int x, int y, int width, int height,
	int total, int view, int pos, int vertical)
{
	struct graphics_device *dev = (struct graphics_device *)device;
	struct ag_ui_node node;
	if (!dev || drv != &agenticos_driver || width <= 0 || height <= 0) return;
	memset(&node, 0, sizeof(node));
	node.version = AG_UI_ABI_VERSION;
	node.byte_len = sizeof(node);
	node.kind = AG_UI_NODE_SCROLLBAR;
	node.flags = vertical ? 1u : 0u;
	node.x = x;
	node.y = y;
	node.width = (uint32_t)width;
	node.height = (uint32_t)height;
	node.value = total;
	node.value_min = view;
	node.value_max = pos;
	ag_ui_control_draw((struct ag_surface *)dev->driver_data, &node);
}

#endif
