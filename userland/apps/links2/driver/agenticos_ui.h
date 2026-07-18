#ifndef LINKS_AGENTICOS_UI_H
#define LINKS_AGENTICOS_UI_H

#include <stddef.h>
#include <stdint.h>

#define AG_UI_ABI_VERSION 1u
#define AG_UI_MAX_MENU_NODES 256u
#define AG_UI_MAX_TEXT_BYTES (1024u * 1024u)

enum ag_ui_node_flags {
	AG_UI_NODE_SEPARATOR = 1u << 0,
	AG_UI_NODE_DISABLED  = 1u << 1,
	AG_UI_NODE_CHECKED   = 1u << 2,
	AG_UI_NODE_RADIO_MARK = 1u << 3,
	AG_UI_NODE_SUBMENU   = 1u << 4,
	AG_UI_NODE_FOCUSED   = 1u << 5
};

enum ag_ui_node_kind {
	AG_UI_NODE_LABEL = 1,
	AG_UI_NODE_CHECKBOX = 2,
	AG_UI_NODE_RADIO = 3,
	AG_UI_NODE_FIELD = 4,
	AG_UI_NODE_PASSWORD = 5,
	AG_UI_NODE_BUTTON = 6,
	AG_UI_NODE_DIALOG = 7,
	AG_UI_NODE_PROGRESS = 8,
	AG_UI_NODE_COMBO = 9,
	AG_UI_NODE_TEXTAREA = 10,
	AG_UI_NODE_SCROLLBAR = 11,
	AG_UI_NODE_TREE_ROW = 12
};

enum ag_ui_action_kind {
	AG_UI_ACTION_NONE = 0,
	AG_UI_ACTION_ACTIVATE = 1,
	AG_UI_ACTION_CANCEL = 2,
	AG_UI_ACTION_CHROME = 3
};

struct ag_ui_text {
	const unsigned char *ptr;
	uint32_t len;
};

struct ag_ui_node {
	uint32_t version;
	uint32_t byte_len;
	uint64_t id;
	uint32_t kind;
	uint32_t flags;
	uint32_t role;
	uint32_t group;
	int32_t x;
	int32_t y;
	uint32_t width;
	uint32_t height;
	int64_t value;
	int64_t value_min;
	int64_t value_max;
	struct ag_ui_text label;
	struct ag_ui_text secondary;
	struct ag_ui_text value_text;
};

struct ag_ui_result {
	uint32_t version;
	uint32_t byte_len;
	uint32_t consumed;
	uint32_t repaint;
	uint32_t action_kind;
	uint32_t flags;
	uint64_t target;
	int64_t value;
};

struct ag_surface;
struct ag_gui_event;
struct terminal;
struct menu_item;
struct dialog_data;

int ag_ui_menu_open(struct ag_surface *, uint64_t, const struct ag_ui_node *,
	size_t, int32_t, int32_t, int32_t);
void ag_ui_close(struct ag_surface *, uint64_t);
void ag_ui_render(struct ag_surface *);
void ag_ui_handle_event(struct ag_surface *, const struct ag_gui_event *,
	struct ag_ui_result *);
void ag_ui_chrome_show(struct ag_surface *, uint32_t);
void ag_ui_chrome_set_status(struct ag_surface *, const unsigned char *, size_t);
void ag_ui_chrome_set_location(struct ag_surface *, const unsigned char *, size_t);
int ag_ui_dialog_update(struct ag_surface *, uint64_t, const struct ag_ui_node *,
	size_t, int32_t);
void ag_ui_dialog_close(struct ag_surface *, uint64_t);
void ag_ui_control_draw(struct ag_surface *, const struct ag_ui_node *);

int agenticos_ui_open_links_menu(struct terminal *, struct menu_item *, void *,
	int, void (*)(void *), void *, int);
void agenticos_ui_accept_result(const struct ag_ui_result *);
void agenticos_ui_shutdown(struct ag_surface *);
int agenticos_ui_set_document_view(void *, int *, int *, int *, int *);
int agenticos_ui_set_status(struct terminal *, const unsigned char *);
int agenticos_ui_set_location(struct terminal *, const unsigned char *);
int agenticos_ui_main_menu_key(struct terminal *, int, int);
void agenticos_ui_dialog_begin(struct dialog_data *);
void agenticos_ui_dialog_label(struct dialog_data *, const unsigned char *, int, int);
void agenticos_ui_dialog_progress(struct dialog_data *, int, int, int, int, int);
void agenticos_ui_dialog_list_row(struct dialog_data *, const unsigned char *,
	int, int, int, int, int, int, int);
void agenticos_ui_dialog_scrollbar(struct dialog_data *, int, int, int, int,
	int, int, int);
void agenticos_ui_dialog_refresh(struct dialog_data *);
void agenticos_ui_dialog_close(struct dialog_data *);
void agenticos_ui_form_control(struct terminal *, int, int, int, int, int,
	const unsigned char *, int, int);
void agenticos_ui_scrollbar(void *, int, int, int, int, int, int, int, int);

#endif
