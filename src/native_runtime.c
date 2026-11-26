/*
 * HackLang native runtime — first supported subset.
 * Values are intentionally dynamic; the Rust checker is not a lowering requirement.
 */
#include <float.h>
#include <math.h>
#include <stdio.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

typedef struct HlArray HlArray;
typedef struct HlMap HlMap;
typedef struct HlMapEntry HlMapEntry;
typedef struct HlClosure HlClosure;
typedef enum { HL_NUMBER, HL_BOOL, HL_STRING, HL_ARRAY, HL_MAP, HL_CLOSURE } HlTag;
typedef enum { HL_STORAGE_SCOPE, HL_STORAGE_OWNED } HlStorageClass;
static int hl_is_aggregate_tag(HlTag tag) { return tag == HL_ARRAY || tag == HL_MAP; }
typedef struct {
    HlTag tag;
    union { double number; int boolean; const char *string; HlArray *array; HlMap *map; HlClosure *closure; } as;
    size_t string_length; /* UTF-8 byte length; strings may contain embedded NUL bytes. */
} HlValue;
struct HlArray {
    size_t length;
    HlValue *items;
};
struct HlMapEntry {
    HlValue key;
    HlValue value;
};
struct HlMap {
    size_t length;
    HlMapEntry *entries;
};

/* Activation-local frame storage. The compiler supplies the slot array;
 * frames are independent across recursive/nested activations. */
typedef struct {
    HlValue value;
    unsigned char initialized;
} HlFrameSlot;

typedef struct {
    HlFrameSlot *slots;
    size_t slot_count;
} HlFrame;

static int hl_frame_init(HlFrame *frame, HlFrameSlot *storage, size_t slot_count) {
    if (!frame || (!storage && slot_count != 0)) return 0;
    frame->slots = storage;
    frame->slot_count = slot_count;
    for (size_t i = 0; i < slot_count; ++i) {
        storage[i].initialized = 0;
    }
    return 1;
}

static int hl_frame_write(HlFrame *frame, size_t slot, HlValue value) {
    if (!frame || !frame->slots || slot >= frame->slot_count) return 0;
    frame->slots[slot].value = value;
    frame->slots[slot].initialized = 1;
    return 1;
}

static int hl_frame_read(const HlFrame *frame, size_t slot, HlValue *out) {
    if (!frame || !frame->slots || !out || slot >= frame->slot_count
        || !frame->slots[slot].initialized) return 0;
    *out = frame->slots[slot].value;
    return 1;
}

typedef struct HlEnv HlEnv;
typedef void (*HlCleanupFn)(void *);

typedef struct HlArenaBlock {
    struct HlArenaBlock *next;
    size_t capacity;
    size_t used;
    max_align_t alignment;
    unsigned char data[];
} HlArenaBlock;

typedef struct HlArenaLane {
    HlArenaBlock *blocks;
    HlArenaBlock *current;
} HlArenaLane;

typedef struct HlOwnershipRecord HlOwnershipRecord;
typedef struct HlCell {
    HlValue value;
    HlEnv *owner;
    HlArenaLane value_lane; /* reclaimable storage for the cell's current value graph */
    int initialized;
    int marked; /* tracing mark, reset by every collection */
    struct HlCell *next;
} HlCell;
struct HlClosure {
    uint32_t function_id;
    size_t capture_count;
    HlCell **captures;
    int marked; /* tracing mark, reset by every collection */
    struct HlClosure *next;

};

/* Root registry: activation frames and protected temporaries are roots of the
 * closure/cell graph. Nodes live in the root environment and are popped in
 * LIFO order by the generated unwinding path. */
typedef struct HlRootFrame {
    HlFrame *frame;
    struct HlRootFrame *next;
} HlRootFrame;
typedef struct HlRootValue {
    HlValue *slot;
    struct HlRootValue *next;
} HlRootValue;
typedef struct HlAllocation {
    HlCleanupFn cleanup;
    void *context;
    struct HlAllocation *next;
    HlOwnershipRecord *ownership;
} HlAllocation;
struct HlOwnershipRecord {
    uint64_t id; /* unique dynamic handle */
    uint64_t site_id; /* stable compiler operation identity */
    HlEnv *owner;
    HlAllocation *allocation;
    int active;
    HlOwnershipRecord *next;
};
typedef struct HlBinding {
    const char *name;
    HlValue value;
    HlEnv *owner;
    char *string_storage;
    size_t string_capacity;
    uint64_t ownership_site;
    uint64_t ownership_token;
    HlArenaLane aggregate_lane;
    HlCell *cell;
    struct HlBinding *next;
} HlBinding;
struct HlEnv {
    HlEnv *parent;
    HlBinding *bindings;
    HlAllocation *allocations;
    HlOwnershipRecord *ownership_records; /* root environment owns token tombstones */
    HlCell *cells; /* all capture cells, swept by tracing collection */
    HlClosure *closures; /* all closure descriptors, swept by tracing collection */
    HlRootFrame *frame_roots; /* active activation frames */
    HlRootValue *value_roots; /* protected evaluator temporaries */
    uint64_t next_ownership_id;
    uint64_t eval_steps;
    uint64_t max_eval_steps;
    uint64_t max_eval_millis;
    clock_t eval_started_at;
    int has_eval_clock;
    uint64_t arena_bytes_reserved;
    uint64_t max_arena_bytes;
    uint64_t heap_bytes_live;
    uint64_t max_heap_bytes;
    int heap_limit_exceeded;
    HlArenaLane *lane;
    HlArenaBlock *lane_mark_block;
    size_t lane_mark_used;
    int owns_lane;
    int closing;
    size_t source_start;
    size_t source_end;
    size_t source_line;
    size_t source_column;
    int has_source_range;
};

static HlEnv *hl_env_root(HlEnv *environment);

typedef struct HlHeapHeader {
    HlEnv *root;
    uint64_t charged_bytes;
    max_align_t alignment;
} HlHeapHeader;

/* Auxiliary heap allocations are charged to the root execution environment.
 * The header preserves the owner so cleanup callbacks can release resources
 * without needing an environment argument. */
static void *hl_heap_alloc(HlEnv *environment, size_t size) {
    if (size > SIZE_MAX - sizeof(HlHeapHeader)) return NULL;
    HlEnv *root = hl_env_root(environment);
    uint64_t charge = (uint64_t)(sizeof(HlHeapHeader) + size);
    if (root && (root->heap_bytes_live > root->max_heap_bytes ||
        charge > root->max_heap_bytes - root->heap_bytes_live)) {
        root->heap_limit_exceeded = 1;
        return NULL;
    }
    HlHeapHeader *header = (HlHeapHeader *)malloc(sizeof(HlHeapHeader) + size);
    if (!header) return NULL;
    header->root = root;
    header->charged_bytes = charge;
    if (root) root->heap_bytes_live += charge;
    return (void *)(header + 1);
}
static void *hl_heap_calloc(HlEnv *environment, size_t count, size_t size) {
    if (count && size > SIZE_MAX / count) return NULL;
    size_t total = count * size;
    void *memory = hl_heap_alloc(environment, total);
    if (memory) memset(memory, 0, total);
    return memory;
}
static void hl_heap_free(void *memory) {
    if (!memory) return;
    HlHeapHeader *header = ((HlHeapHeader *)memory) - 1;
    if (header->root) {
        if (header->root->heap_bytes_live >= header->charged_bytes)
            header->root->heap_bytes_live -= header->charged_bytes;
        else
            header->root->heap_bytes_live = 0;
    }
    free(header);
}
static void *hl_heap_realloc(HlEnv *environment, void *memory, size_t size) {
    if (!memory) return hl_heap_alloc(environment, size);
    if (size == 0) { hl_heap_free(memory); return NULL; }
    if (size > SIZE_MAX - sizeof(HlHeapHeader)) return NULL;
    HlHeapHeader *old_header = ((HlHeapHeader *)memory) - 1;
    HlEnv *root = old_header->root;
    uint64_t new_charge = (uint64_t)(sizeof(HlHeapHeader) + size);
    uint64_t old_charge = old_header->charged_bytes;
    if (root && new_charge > old_charge &&
        (root->heap_bytes_live > root->max_heap_bytes ||
         new_charge - old_charge > root->max_heap_bytes - root->heap_bytes_live)) {
        root->heap_limit_exceeded = 1;
        return NULL;
    }
    HlHeapHeader *new_header = (HlHeapHeader *)realloc(old_header, sizeof(HlHeapHeader) + size);
    if (!new_header) return NULL;
    new_header->root = root;
    new_header->charged_bytes = new_charge;
    if (root) {
        if (new_charge >= old_charge) root->heap_bytes_live += new_charge - old_charge;
        else root->heap_bytes_live -= old_charge - new_charge;
    }
    (void)environment;
    return (void *)(new_header + 1);
}

static void hl_lane_init(HlArenaLane *lane);
static void hl_lane_destroy(HlArenaLane *lane);

/* Binding-owned dynamic storage participates in the same exactly-once
 * cleanup registry as other runtime resources. The binding descriptor stays
 * alive until callbacks have run and its arena is rewound. */
static void hl_cleanup_binding_string(void *context) {
    HlBinding *binding = (HlBinding *)context;
    if (!binding) return;
    hl_heap_free(binding->string_storage);
    binding->string_storage = NULL;
    binding->string_capacity = 0;
    binding->ownership_token = 0;
}

static void hl_cleanup_binding_aggregate(void *context) {
    HlBinding *binding = (HlBinding *)context;
    if (!binding) return;
    hl_lane_destroy(&binding->aggregate_lane);
    binding->value.tag = HL_NUMBER;
    binding->value.as.number = 0.0;
    binding->ownership_token = 0;
}

static void hl_env_free(HlEnv *environment);

static void hl_json_write_string(FILE *stream, const char *value) {
    fputc('"', stream);
    for (const unsigned char *cursor = (const unsigned char *)value; *cursor; ++cursor) {
        const unsigned char ch = *cursor;
        switch (ch) {
            case '"': fputs("\\\"", stream); break;
            case '\\': fputs("\\\\", stream); break;
            case '\n': fputs("\\n", stream); break;
            case '\r': fputs("\\r", stream); break;
            case '\t': fputs("\\t", stream); break;
            default:
                if (ch < 0x20) fprintf(stream, "\\u%04x", (unsigned int)ch);
                else fputc(ch, stream);
                break;
        }
    }
    fputc('"', stream);
}

static void hl_set_source_range(HlEnv *environment, size_t start, size_t end,
                                size_t line, size_t column) {
    if (!environment) return;
    environment->source_start = start;
    environment->source_end = end;
    environment->source_line = line;
    environment->source_column = column;
    environment->has_source_range = 1;
}

static void hl_clear_source_range(HlEnv *environment) {
    if (environment) environment->has_source_range = -1;
}

static void hl_fail(HlEnv *environment, const char *message) {
    HlEnv *root = hl_env_root(environment);
    if (root && root->heap_limit_exceeded)
        message = "se agotó el presupuesto de memoria auxiliar del runtime nativo";
    const HlEnv *source = environment;
    while (source && source->has_source_range == 0) source = source->parent;
    const char *diagnostics = getenv("HACKLANG_DIAGNOSTICS");
    if (diagnostics && strcmp(diagnostics, "json") == 0) {
        fputs("[{\"code\":\"HL-RUNTIME-001\",\"phase\":\"runtime\","
              "\"severity\":\"error\",\"message\":", stderr);
        hl_json_write_string(stderr, message ? message : "Error de ejecución desconocido");
        if (source && source->has_source_range > 0) {
            fprintf(stderr, ",\"range\":{\"start\":%zu,\"end\":%zu,\"line\":%zu,\"column\":%zu}}]\n",
                    source->source_start, source->source_end,
                    source->source_line, source->source_column);
        } else {
            fputs(",\"range\":null}]\n", stderr);
        }
    } else {
        fprintf(stderr, "Error en tiempo de ejecución: %s\n", message);
    }
    while (environment) {
        HlEnv *parent = environment->parent;
        hl_env_free(environment);
        environment = parent;
    }
    exit(1);
}

static HlValue hl_frame_read_or_fail(HlEnv *env, const HlFrame *frame, size_t slot) {
    HlValue value;
    if (!hl_frame_read(frame, slot, &value))
        hl_fail(env, "lectura de slot de frame no inicializado o fuera de rango");
    return value;
}

static HlValue hl_clone_value(HlEnv *environment, HlValue value);

static void hl_frame_write_or_fail(HlEnv *env, HlEnv *frame_owner,
        HlFrame *frame, size_t slot, HlValue value) {
    /* Frame slots live for the whole activation, not the current lexical
     * scope. Promote into the activation's lane before storing the value. */
    HlValue stable = hl_clone_value(frame_owner, value);
    if (!hl_frame_write(frame, slot, stable))
        hl_fail(env, "escritura de slot de frame fuera de rango");
}

/* Register one cleanup action. The returned handle remains stable until
 * cancellation, transfer, or execution by hl_env_free. */
HlAllocation *hl_env_defer(HlEnv *environment, HlCleanupFn cleanup, void *context) {
    if (!environment || environment->closing || !cleanup) return NULL;
    HlAllocation *entry = (HlAllocation *)hl_heap_alloc(environment, sizeof(HlAllocation));
    if (!entry) return NULL;
    entry->cleanup = cleanup;
    entry->context = context;
    entry->ownership = NULL;
    entry->next = environment->allocations;
    environment->allocations = entry;
    return entry;
}

static int hl_cleanup_detach(HlEnv *environment, HlAllocation *entry) {
    if (!environment || !entry) return 0;
    HlAllocation **cursor = &environment->allocations;
    while (*cursor && *cursor != entry) cursor = &(*cursor)->next;
    if (!*cursor) return 0;
    *cursor = entry->next;
    entry->next = NULL;
    return 1;
}
static void hl_ownership_invalidate_allocation(HlAllocation *entry) {
    if (!entry || !entry->ownership) return;
    entry->ownership->active = 0;
    entry->ownership->owner = NULL;
    entry->ownership->allocation = NULL;
    entry->ownership = NULL;
}

/* A transfer may only extend a registration to the source environment's
 * lexical ancestor. This runtime cannot prove arbitrary owner lifetimes. */
static int hl_env_is_ancestor(HlEnv *ancestor, HlEnv *environment) {
    for (HlEnv *scope = environment; scope; scope = scope->parent)
        if (scope == ancestor) return 1;
    return 0;
}

/* Transfer an existing registration; never clone the cleanup action. */
int hl_env_transfer(HlEnv *from, HlEnv *to, HlAllocation *entry) {
    if (!from || !to || from == to || from->closing || to->closing
        || !hl_env_is_ancestor(to, from) || !hl_cleanup_detach(from, entry)) return 0;
    entry->next = to->allocations;
    to->allocations = entry;
    if (entry->ownership) entry->ownership->owner = to;
    return 1;
}

/* Cancel ownership without running the callback. The caller must already
 * have handled or transferred the resource. */
int hl_env_cancel(HlEnv *environment, HlAllocation *entry) {
    if (!hl_cleanup_detach(environment, entry)) return 0;
    hl_ownership_invalidate_allocation(entry);
    hl_heap_free(entry);
    return 1;
}


/* Ownership tokens are stable integer identities, not allocation pointers.
 * Their tombstones are owned by the root environment; a child cleanup
 * invalidates the token before invoking its callback, while the tombstone
 * remains queryable until the root itself is destroyed. */
static HlEnv *hl_env_root(HlEnv *environment) {
    if (!environment) return NULL;
    while (environment->parent) environment = environment->parent;
    return environment;
}

/* Charge one unit of native execution budget. The root owns the counter so
 * nested scopes and function activations share a single program-wide limit. */
static void hl_eval_step(HlEnv *environment) {
    HlEnv *root = hl_env_root(environment);
    if (!root) return;
    if (root->eval_steps >= root->max_eval_steps)
        hl_fail(environment, "se agotó el presupuesto de instrucciones de ejecución nativa");
    if (root->has_eval_clock) {
        clock_t now = clock();
        if (now == (clock_t)-1 || now < root->eval_started_at)
            hl_fail(environment, "no se pudo consultar el reloj de ejecución nativo");
        uint64_t elapsed_ticks = (uint64_t)(now - root->eval_started_at);
        uint64_t max_ticks;
        uint64_t whole_seconds = root->max_eval_millis / UINT64_C(1000);
        uint64_t remaining_millis = root->max_eval_millis % UINT64_C(1000);
        if (whole_seconds > UINT64_MAX / (uint64_t)CLOCKS_PER_SEC) {
            max_ticks = UINT64_MAX;
        } else {
            max_ticks = whole_seconds * (uint64_t)CLOCKS_PER_SEC;
            uint64_t fraction = (remaining_millis * (uint64_t)CLOCKS_PER_SEC + UINT64_C(999)) / UINT64_C(1000);
            if (fraction > UINT64_MAX - max_ticks) max_ticks = UINT64_MAX;
            else max_ticks += fraction;
        }
        if (elapsed_ticks >= max_ticks)
            hl_fail(environment, "se agotó el presupuesto de tiempo de CPU de ejecución nativa");
    }
    ++root->eval_steps;
}
static HlOwnershipRecord *hl_ownership_find(HlEnv *environment, uint64_t id) {
    HlEnv *root = hl_env_root(environment);
    if (!root) return NULL;
    for (HlOwnershipRecord *record = root->ownership_records; record; record = record->next)
        if (record->id == id) return record;
    return NULL;
}
int hl_ownership_acquire(HlEnv *environment, uint64_t site_id, uint64_t *token_out, HlCleanupFn cleanup, void *context) {
    HlEnv *root = hl_env_root(environment);
    if (!root || !environment || environment->closing || !token_out || !cleanup
        || root->next_ownership_id == UINT64_MAX) return 0;
    HlOwnershipRecord *record = (HlOwnershipRecord *)hl_heap_alloc(environment, sizeof(HlOwnershipRecord));
    if (!record) return 0;
    HlAllocation *allocation = hl_env_defer(environment, cleanup, context);
    if (!allocation) { hl_heap_free(record); return 0; }
    record->id = ++root->next_ownership_id;
    record->site_id = site_id;
    record->owner = environment;
    record->allocation = allocation;
    record->active = 1;
    record->next = root->ownership_records;
    root->ownership_records = record;
    allocation->ownership = record;
    *token_out = record->id;
    return 1;
}
int hl_ownership_transfer(HlEnv *from, HlEnv *to, uint64_t id) {
    HlOwnershipRecord *record = hl_ownership_find(from, id);
    if (!record || !record->active || record->owner != from || !record->allocation) return 0;
    return hl_env_transfer(from, to, record->allocation);
}
int hl_ownership_drop(HlEnv *environment, uint64_t id) {
    HlOwnershipRecord *record = hl_ownership_find(environment, id);
    if (!record || !record->active || record->owner != environment || !record->allocation) return 0;
    return hl_env_cancel(environment, record->allocation);
}
int hl_ownership_is_active(HlEnv *environment, uint64_t id) {
    HlOwnershipRecord *record = hl_ownership_find(environment, id);
    return record && record->active && record->owner != NULL && record->allocation != NULL;
}

static void hl_lane_init(HlArenaLane *lane) {
    lane->blocks = NULL; lane->current = NULL;
}
static void hl_lane_destroy(HlArenaLane *lane) {
    if (!lane) return;
    HlArenaBlock *block = lane->blocks;
    while (block) { HlArenaBlock *next = block->next; free(block); block = next; }
    lane->blocks = NULL; lane->current = NULL;
}
static void hl_lane_rewind(HlArenaLane *lane, HlArenaBlock *mark, size_t used) {
    if (!lane) return;
    HlArenaBlock *block = mark ? mark->next : lane->blocks;
    while (block) { block->used = 0; block = block->next; }
    if (mark) { mark->used = used; lane->current = mark; }
    else { lane->current = lane->blocks; }
}
static void *hl_alloc(HlEnv *environment, size_t size) {
    if (!environment || environment->closing || !environment->lane)
        hl_fail(environment, "región de memoria no disponible");
    const size_t alignment = _Alignof(max_align_t);
    if (size == 0) size = 1;
    if (size > SIZE_MAX - (alignment - 1))
        hl_fail(environment, "tamaño de asignación fuera de rango");
    size_t required = (size + alignment - 1) & ~(alignment - 1);
    HlArenaLane *lane = environment->lane;
    HlArenaBlock *block = lane->current ? lane->current : lane->blocks;
    while (block && required > block->capacity - block->used) block = block->next;
    if (!block) {
        const size_t default_capacity = 4096;
        size_t capacity = required > default_capacity ? required : default_capacity;
        if (capacity > SIZE_MAX - sizeof(HlArenaBlock))
            hl_fail(environment, "tamaño de región fuera de rango");
        HlEnv *root = hl_env_root(environment);
        if (root && (root->arena_bytes_reserved > root->max_arena_bytes ||
            (uint64_t)capacity > root->max_arena_bytes - root->arena_bytes_reserved))
            hl_fail(environment, "se agotó el presupuesto de memoria de arenas nativas");
        block = (HlArenaBlock *)malloc(sizeof(HlArenaBlock) + capacity);
        if (!block) hl_fail(environment, "memoria agotada");
        if (root) root->arena_bytes_reserved += (uint64_t)capacity;
        block->next = NULL; block->capacity = capacity; block->used = 0;
        if (!lane->blocks) lane->blocks = block;
        else { HlArenaBlock *tail = lane->blocks; while (tail->next) tail = tail->next; tail->next = block; }
    }
    lane->current = block;
    void *memory = block->data + block->used;
    block->used += required;
    return memory;
}

static void hl_env_free(HlEnv *environment) {
    if (!environment || environment->closing) return;
    environment->closing = 1;
    HlAllocation *allocation = environment->allocations;
    environment->allocations = NULL;
    while (allocation) {
        HlAllocation *next = allocation->next;
        hl_ownership_invalidate_allocation(allocation);
        allocation->cleanup(allocation->context);
        hl_heap_free(allocation);
        allocation = next;
    }
    /* Token tombstones live in the root until all descendant callbacks have run. */
    if (!environment->parent) {
        HlRootFrame *frame_root = environment->frame_roots;
        while (frame_root) { HlRootFrame *next = frame_root->next; hl_heap_free(frame_root); frame_root = next; }
        environment->frame_roots = NULL;
        HlRootValue *value_root = environment->value_roots;
        while (value_root) { HlRootValue *next = value_root->next; hl_heap_free(value_root); value_root = next; }
        environment->value_roots = NULL;
        HlClosure *closure = environment->closures;
        while (closure) { HlClosure *next = closure->next; hl_heap_free(closure->captures); hl_heap_free(closure); closure = next; }
        environment->closures = NULL;
        HlCell *cell = environment->cells;
        while (cell) { HlCell *next = cell->next; hl_lane_destroy(&cell->value_lane); hl_heap_free(cell); cell = next; }
        environment->cells = NULL;
        HlOwnershipRecord *record = environment->ownership_records;
        while (record) { HlOwnershipRecord *next = record->next; hl_heap_free(record); record = next; }
        environment->ownership_records = NULL;
    }
    /* Binding-owned buffers are released by registered callbacks above. */
    if (environment->owns_lane) {
        hl_lane_destroy(environment->lane);
        free(environment->lane);
    } else {
        hl_lane_rewind(environment->lane, environment->lane_mark_block, environment->lane_mark_used);
    }
    free(environment);
}
static HlValue hl_clone_value(HlEnv *environment, HlValue value);
static HlValue hl_clone_value_in_lane(HlEnv *environment, HlArenaLane *lane, HlValue value);

/* Captured cells have stable heap identity. Their current value graph has a
 * private lane, so overwrites and tracing-GC sweeps reclaim it independently. */
static HlCell *hl_cell_new(HlEnv *environment, HlValue initial, int initialized) {
    HlEnv *root = hl_env_root(environment);
    if (!root || root->closing) return NULL;
    HlCell *cell = (HlCell *)hl_heap_alloc(environment, sizeof(HlCell));
    if (!cell) hl_fail(environment, "memoria agotada al crear celda capturada");
    cell->value = initial;
    cell->owner = root;
    hl_lane_init(&cell->value_lane);
    cell->initialized = initialized != 0;
    cell->marked = 0;
    cell->next = root->cells;
    root->cells = cell;
    if (initialized)
        cell->value = hl_clone_value_in_lane(root, &cell->value_lane, initial);
    return cell;
}
static int hl_cell_read(const HlCell *cell, HlValue *out) {
    if (!cell || !out || !cell->initialized) return 0;
    *out = cell->value;
    return 1;
}
static int hl_cell_write(HlCell *cell, HlValue value) {
    if (!cell) return 0;
    if (!cell->owner || cell->owner->closing) return 0;
    /* Replace the old graph as a unit. If cloning fails, the cell is already
     * registered on the root and teardown can still reclaim its fresh lane. */
    hl_lane_destroy(&cell->value_lane);
    hl_lane_init(&cell->value_lane);
    cell->value = hl_clone_value_in_lane(cell->owner, &cell->value_lane, value);
    cell->initialized = 1;
    return 1;
}

static void *hl_alloc(HlEnv *environment, size_t size);
static char *hl_copy_string(HlEnv *environment, const char *value);
static HlBinding *hl_find_local(HlEnv *environment, const char *name);
static HlBinding *hl_find(HlEnv *environment, const char *name);

static HlCell *hl_closure_capture_cell(HlValue closure, size_t index) {
    if (closure.tag != HL_CLOSURE || !closure.as.closure
        || index >= closure.as.closure->capture_count)
        return NULL;
    return closure.as.closure->captures[index];
}
static void hl_binding_alias_cell(HlEnv *environment, const char *name, HlCell *cell) {
    if (!environment || !cell || !cell->initialized)
        hl_fail(environment, "celda de captura no disponible al entrar en función");
    if (hl_find_local(environment, name))
        hl_fail(environment, "binding duplicado al instalar captura");
    HlBinding *binding = (HlBinding *)hl_alloc(environment, sizeof(HlBinding));
    binding->name = hl_copy_string(environment, name);
    binding->owner = environment;
    binding->string_storage = NULL;
    binding->string_capacity = 0;
    binding->ownership_site = 0;
    binding->ownership_token = 0;
    binding->aggregate_lane = (HlArenaLane){0};
    binding->value = cell->value;
    binding->cell = cell;
    binding->next = environment->bindings;
    environment->bindings = binding;
}
static HlCell *hl_binding_capture(HlEnv *environment, const char *name) {
    HlBinding *binding = hl_find(environment, name);
    if (!binding) {
        char message[512];
        snprintf(message, sizeof(message), "Variable '%s' no definida para captura", name);
        hl_fail(environment, message);
    }
    if (!binding->cell) binding->cell = hl_cell_new(binding->owner, binding->value, 1);
    return binding->cell;
}
/* A captured binding must exist before its initializer runs: a self-recursive
 * lambda captures the binding it is currently defining. Predeclaring installs
 * the binding and its (still uninitialized) cell, so capture creation finds the
 * same identity that the definition later initializes. */
static void hl_binding_predeclare(HlEnv *environment, const char *name, uint64_t ownership_site) {
    if (!environment || environment->closing)
        hl_fail(environment, "entorno cerrado al predeclarar un binding");
    if (hl_find_local(environment, name)) return;
    HlValue zero; zero.tag = HL_NUMBER; zero.as.number = 0.0;
    HlBinding *binding = (HlBinding *)hl_alloc(environment, sizeof(HlBinding));
    binding->name = hl_copy_string(environment, name);
    binding->owner = environment;
    binding->string_storage = NULL;
    binding->string_capacity = 0;
    binding->ownership_site = ownership_site;
    binding->ownership_token = 0;
    binding->aggregate_lane = (HlArenaLane){0};
    binding->value = zero;
    binding->cell = hl_cell_new(environment, zero, 0);
    binding->next = environment->bindings;
    environment->bindings = binding;
}
static HlValue hl_closure_new(HlEnv *environment, uint32_t function_id,
        const char *const *capture_names, size_t capture_count) {
    HlEnv *root = hl_env_root(environment);
    if (!root || root->closing) hl_fail(environment, "entorno raíz no disponible para closure");
    HlClosure *closure = (HlClosure *)hl_heap_calloc(environment, 1, sizeof(HlClosure));
    if (!closure) hl_fail(environment, "memoria agotada al crear closure");
    if (capture_count) {
        closure->captures = (HlCell **)hl_heap_calloc(environment, capture_count, sizeof(HlCell *));
        if (!closure->captures) { hl_heap_free(closure); hl_fail(environment, "memoria agotada al capturar bindings"); }
    }
    closure->function_id = function_id;
    closure->capture_count = capture_count;
    for (size_t i = 0; i < capture_count; ++i)
        closure->captures[i] = hl_binding_capture(environment, capture_names[i]);
    closure->next = root->closures;
    root->closures = closure;
    HlValue result = {0}; result.tag = HL_CLOSURE; result.as.closure = closure; return result;
}
static int hl_closure_capture_read(const HlValue value, size_t index, HlValue *out) {
    if (value.tag != HL_CLOSURE || !value.as.closure
        || index >= value.as.closure->capture_count) return 0;
    return hl_cell_read(value.as.closure->captures[index], out);
}
static int hl_closure_capture_write(const HlValue value, size_t index, HlValue captured) {
    if (value.tag != HL_CLOSURE || !value.as.closure
        || index >= value.as.closure->capture_count) return 0;
    return hl_cell_write(value.as.closure->captures[index], captured);
}

/* --- Root registry and cycle tracing ------------------------------------
 * Cells and closures form a graph (cell -> closure -> captured cell). Cells
 * meant for a self- or mutually-recursive closure would otherwise keep each
 * other alive forever. The generated code registers every activation frame and
 * every protected temporary as a root; collection marks the graph from the
 * live environment chain plus those roots and sweeps the rest. It runs only at
 * statement boundaries, where no evaluator temporary is live.
 */
static void hl_trace_value(HlValue value);
static void hl_trace_cell(HlCell *cell);
static void hl_trace_closure(HlClosure *closure);

static void hl_trace_value(HlValue value) {
    switch (value.tag) {
        case HL_CLOSURE: hl_trace_closure(value.as.closure); break;
        case HL_ARRAY:
            if (value.as.array)
                for (size_t i = 0; i < value.as.array->length; ++i)
                    hl_trace_value(value.as.array->items[i]);
            break;
        case HL_MAP:
            if (value.as.map)
                for (size_t i = 0; i < value.as.map->length; ++i) {
                    hl_trace_value(value.as.map->entries[i].key);
                    hl_trace_value(value.as.map->entries[i].value);
                }
            break;
        default: break;
    }
}
static void hl_trace_cell(HlCell *cell) {
    if (!cell || cell->marked) return;
    cell->marked = 1;
    if (cell->initialized) hl_trace_value(cell->value);
}
static void hl_trace_closure(HlClosure *closure) {
    if (!closure || closure->marked) return;
    closure->marked = 1;
    for (size_t i = 0; i < closure->capture_count; ++i)
        hl_trace_cell(closure->captures[i]);
}

static void hl_root_frame_push(HlEnv *environment, HlFrame *frame) {
    HlEnv *root = hl_env_root(environment);
    if (!root || root->closing || !frame) return;
    HlRootFrame *node = (HlRootFrame *)hl_heap_alloc(environment, sizeof(HlRootFrame));
    if (!node) hl_fail(environment, "memoria agotada al registrar el frame activo");
    node->frame = frame;
    node->next = root->frame_roots;
    root->frame_roots = node;
}
static void hl_root_frame_pop(HlEnv *environment) {
    HlEnv *root = hl_env_root(environment);
    if (!root || !root->frame_roots) return;
    HlRootFrame *node = root->frame_roots;
    root->frame_roots = node->next;
    hl_heap_free(node);
}
static void hl_root_value_push(HlEnv *environment, HlValue *slot) {
    HlEnv *root = hl_env_root(environment);
    if (!root || root->closing || !slot) return;
    HlRootValue *node = (HlRootValue *)hl_heap_alloc(environment, sizeof(HlRootValue));
    if (!node) hl_fail(environment, "memoria agotada al registrar una raíz temporal");
    node->slot = slot;
    node->next = root->value_roots;
    root->value_roots = node;
}
static void hl_root_value_pop(HlEnv *environment, HlValue *slot) {
    HlEnv *root = hl_env_root(environment);
    if (!root) return;
    HlRootValue **link = &root->value_roots;
    while (*link && (*link)->slot != slot) link = &(*link)->next;
    if (!*link) return;
    HlRootValue *node = *link;
    *link = node->next;
    hl_heap_free(node);
}

static void hl_collect(HlEnv *environment) {
    HlEnv *root = hl_env_root(environment);
    if (!root || root->closing) return;
    for (HlEnv *scope = environment; scope; scope = scope->parent)
        for (HlBinding *binding = scope->bindings; binding; binding = binding->next) {
            hl_trace_cell(binding->cell);
            hl_trace_value(binding->value);
        }
    for (HlRootFrame *node = root->frame_roots; node; node = node->next) {
        HlFrame *frame = node->frame;
        if (!frame || !frame->slots) continue;
        for (size_t i = 0; i < frame->slot_count; ++i)
            if (frame->slots[i].initialized) hl_trace_value(frame->slots[i].value);
    }
    for (HlRootValue *node = root->value_roots; node; node = node->next)
        if (node->slot) hl_trace_value(*node->slot);

    HlCell **cell_link = &root->cells;
    while (*cell_link) {
        HlCell *cell = *cell_link;
        if (!cell->marked) { *cell_link = cell->next; hl_lane_destroy(&cell->value_lane); hl_heap_free(cell); continue; }
        cell->marked = 0;
        cell_link = &cell->next;
    }
    HlClosure **closure_link = &root->closures;
    while (*closure_link) {
        HlClosure *closure = *closure_link;
        if (!closure->marked) {
            *closure_link = closure->next;
            hl_heap_free(closure->captures);
            hl_heap_free(closure);
            continue;
        }
        closure->marked = 0;
        closure_link = &closure->next;
    }
}

static char *hl_copy_string_n(HlEnv *environment, const char *value, size_t length) {
    if (length == SIZE_MAX) hl_fail(environment, "cadena demasiado grande");
    char *copy = (char *)hl_alloc(environment, length + 1);
    if (length) memcpy(copy, value, length);
    copy[length] = '\0';
    return copy;
}
static char *hl_copy_string(HlEnv *environment, const char *value) {
    return hl_copy_string_n(environment, value, strlen(value));
}

static HlValue hl_number(double value) {
    HlValue result = {0}; result.tag = HL_NUMBER; result.as.number = value; return result;
}
static HlValue hl_bool(int value) {
    HlValue result = {0}; result.tag = HL_BOOL; result.as.boolean = value != 0; return result;
}
static HlValue hl_string(HlEnv *environment, const char *value, size_t length) {
    HlValue result = {0};
    result.tag = HL_STRING;
    result.as.string = hl_copy_string_n(environment, value, length);
    result.string_length = length;
    return result;
}

/* Aggregate storage is lane-owned. Cloning recursively duplicates the value
 * graph, including strings, so reads/assignments cannot create shared aliases. */
static int hl_map_key_equal(HlValue left, HlValue right) {
    if (left.tag != right.tag) return 0;
    switch (left.tag) {
        case HL_NUMBER: return left.as.number == right.as.number;
        case HL_BOOL: return left.as.boolean == right.as.boolean;
        case HL_STRING:
            return left.as.string && right.as.string
                && left.string_length == right.string_length
                && (left.string_length == 0
                    || memcmp(left.as.string, right.as.string, left.string_length) == 0);
        case HL_ARRAY:
            if (!left.as.array || !right.as.array
                || left.as.array->length != right.as.array->length) return 0;
            for (size_t i = 0; i < left.as.array->length; ++i)
                if (!hl_map_key_equal(left.as.array->items[i], right.as.array->items[i])) return 0;
            return 1;
        case HL_CLOSURE: return left.as.closure == right.as.closure;
        case HL_MAP:
            if (!left.as.map || !right.as.map
                || left.as.map->length != right.as.map->length) return 0;
            for (size_t i = 0; i < left.as.map->length; ++i) {
                if (!hl_map_key_equal(left.as.map->entries[i].key, right.as.map->entries[i].key)
                    || !hl_map_key_equal(left.as.map->entries[i].value, right.as.map->entries[i].value)) return 0;
            }
            return 1;
    }
    return 0;
}
static size_t hl_map_find_key(const HlMap *map, HlValue key) {
    if (!map) return SIZE_MAX;
    for (size_t i = 0; i < map->length; ++i)
        if (hl_map_key_equal(map->entries[i].key, key)) return i;
    return SIZE_MAX;
}
static HlValue hl_map(HlEnv *environment, const HlMapEntry *entries, size_t length) {
    if (length > SIZE_MAX / sizeof(HlMapEntry))
        hl_fail(environment, "mapa demasiado grande");
    HlMap *map = (HlMap *)hl_alloc(environment, sizeof(HlMap));
    map->length = length;
    map->entries = length ? (HlMapEntry *)hl_alloc(environment, length * sizeof(HlMapEntry)) : NULL;
    if (length) memcpy(map->entries, entries, length * sizeof(HlMapEntry));
    HlValue result = {0}; result.tag = HL_MAP; result.as.map = map; return result;
}
static HlValue hl_array(HlEnv *environment, const HlValue *items, size_t length) {
    if (length > SIZE_MAX / sizeof(HlValue))
        hl_fail(environment, "array demasiado grande");
    HlArray *array = (HlArray *)hl_alloc(environment, sizeof(HlArray));
    array->length = length;
    array->items = length
        ? (HlValue *)hl_alloc(environment, length * sizeof(HlValue)) : NULL;
    for (size_t i = 0; i < length; ++i) array->items[i] = items[i];
    HlValue result = {0}; result.tag = HL_ARRAY; result.as.array = array; return result;
}
static HlValue hl_clone_value(HlEnv *environment, HlValue value) {
    switch (value.tag) {
        case HL_NUMBER: return hl_number(value.as.number);
        case HL_BOOL: return hl_bool(value.as.boolean);
        case HL_STRING: return hl_string(environment, value.as.string, value.string_length);
        case HL_ARRAY: {
            if (!value.as.array) hl_fail(environment, "array nulo");
            if (value.as.array->length > SIZE_MAX / sizeof(HlValue))
                hl_fail(environment, "array demasiado grande al copiar");
            HlArray *copy = (HlArray *)hl_alloc(environment, sizeof(HlArray));
            copy->length = value.as.array->length;
            copy->items = copy->length
                ? (HlValue *)hl_alloc(environment, copy->length * sizeof(HlValue)) : NULL;
            for (size_t i = 0; i < copy->length; ++i)
                copy->items[i] = hl_clone_value(environment, value.as.array->items[i]);
            HlValue result = {0}; result.tag = HL_ARRAY; result.as.array = copy; return result;
        }
        case HL_CLOSURE: return value;
        case HL_MAP: {
            if (!value.as.map) hl_fail(environment, "mapa nulo");
            if (value.as.map->length > SIZE_MAX / sizeof(HlMapEntry))
                hl_fail(environment, "mapa demasiado grande al copiar");
            HlMap *copy = (HlMap *)hl_alloc(environment, sizeof(HlMap));
            copy->length = value.as.map->length;
            copy->entries = copy->length
                ? (HlMapEntry *)hl_alloc(environment, copy->length * sizeof(HlMapEntry)) : NULL;
            for (size_t i = 0; i < copy->length; ++i) {
                copy->entries[i].key = hl_clone_value(environment, value.as.map->entries[i].key);
                copy->entries[i].value = hl_clone_value(environment, value.as.map->entries[i].value);
            }
            HlValue result = {0}; result.tag = HL_MAP; result.as.map = copy; return result;
        }
    }
    hl_fail(environment, "tipo de valor desconocido al copiar");
    return hl_number(0);
}
static void *hl_alloc_in_lane(HlEnv *environment, HlArenaLane *lane, size_t size) {
    HlArenaLane *previous_lane = environment->lane;
    environment->lane = lane;
    void *memory = hl_alloc(environment, size);
    environment->lane = previous_lane;
    return memory;
}
static HlValue hl_clone_value_in_lane(HlEnv *environment, HlArenaLane *lane, HlValue value) {
    HlArenaLane *previous_lane = environment->lane;
    environment->lane = lane;
    HlValue copy = hl_clone_value(environment, value);
    environment->lane = previous_lane;
    return copy;
}
static HlEnv *hl_env_new_in_lane(HlEnv *parent, HlArenaLane *lane) {
    if (!lane) hl_fail(parent, "lane de memoria nula");
    HlEnv *environment = (HlEnv *)malloc(sizeof(HlEnv));
    if (!environment) hl_fail(parent, "memoria agotada");
    environment->parent = parent;
    environment->bindings = NULL;
    environment->allocations = NULL;
    environment->ownership_records = NULL;
    environment->cells = NULL;
    environment->closures = NULL;
    environment->frame_roots = NULL;
    environment->value_roots = NULL;
    environment->next_ownership_id = 0;
    environment->eval_steps = 0;
    environment->max_eval_steps = UINT64_C(1000000);
    environment->max_eval_millis = UINT64_C(30000);
    environment->eval_started_at = (clock_t)0;
    environment->has_eval_clock = 0;
    environment->arena_bytes_reserved = 0;
    environment->max_arena_bytes = UINT64_C(268435456);
    environment->heap_bytes_live = 0;
    environment->max_heap_bytes = UINT64_C(268435456);
    environment->heap_limit_exceeded = 0;
    if (!parent) {
        const char *limit = getenv("HACKLANG_MAX_EVAL_STEPS");
        if (limit && *limit >= '0' && *limit <= '9') {
            char *end = NULL;
            unsigned long long parsed = strtoull(limit, &end, 10);
            if (end != limit && *end == '\0' && parsed > 0)
                environment->max_eval_steps = (uint64_t)parsed;
        }
        const char *time_limit = getenv("HACKLANG_MAX_EVAL_MILLIS");
        if (time_limit && *time_limit >= '0' && *time_limit <= '9') {
            char *end = NULL;
            unsigned long long parsed = strtoull(time_limit, &end, 10);
            if (end != time_limit && *end == '\0' && parsed > 0)
                environment->max_eval_millis = (uint64_t)parsed;
        }
        environment->eval_started_at = clock();
        if (environment->eval_started_at == (clock_t)-1)
            hl_fail(environment, "no se pudo iniciar el reloj de ejecución nativo");
        environment->has_eval_clock = 1;
        const char *memory_limit = getenv("HACKLANG_MAX_ARENA_BYTES");
        if (memory_limit && *memory_limit >= '0' && *memory_limit <= '9') {
            char *end = NULL;
            unsigned long long parsed = strtoull(memory_limit, &end, 10);
            if (end != memory_limit && *end == '\0' && parsed > 0)
                environment->max_arena_bytes = (uint64_t)parsed;
        }
        const char *heap_limit = getenv("HACKLANG_MAX_HEAP_BYTES");
        if (heap_limit && *heap_limit >= '0' && *heap_limit <= '9') {
            char *end = NULL;
            unsigned long long parsed = strtoull(heap_limit, &end, 10);
            if (end != heap_limit && *end == '\0' && parsed > 0)
                environment->max_heap_bytes = (uint64_t)parsed;
        }
    }
    environment->lane = lane;
    environment->lane_mark_block = lane->current ? lane->current : lane->blocks;
    environment->lane_mark_used = environment->lane_mark_block ? environment->lane_mark_block->used : 0;
    environment->owns_lane = 0;
    environment->closing = 0;
    return environment;
}
static HlEnv *hl_env_new(HlEnv *parent) {
    HlArenaLane *lane = (HlArenaLane *)malloc(sizeof(HlArenaLane));
    if (!lane) hl_fail(parent, "memoria agotada");
    hl_lane_init(lane);
    HlEnv *environment = hl_env_new_in_lane(parent, lane);
    environment->owns_lane = 1;
    return environment;
}

static HlBinding *hl_find_local(HlEnv *environment, const char *name) {
    for (HlBinding *item = environment->bindings; item; item = item->next)
        if (strcmp(item->name, name) == 0) return item;
    return NULL;
}
static HlBinding *hl_find(HlEnv *environment, const char *name) {
    for (HlEnv *scope = environment; scope; scope = scope->parent) {
        HlBinding *item = hl_find_local(scope, name);
        if (item) return item;
    }
    return NULL;
}
/* Store a value in its binding. Strings use one growable owner buffer,
 * avoiding one permanent arena allocation per reassignment. Copy through a
 * temporary first because the input may alias this binding's current buffer. */
static void hl_release_binding_resource(HlBinding *binding, const char *error_message) {
    if (!binding || !binding->ownership_token) return;
    uint64_t token = binding->ownership_token;
    HlOwnershipRecord *record = hl_ownership_find(binding->owner, token);
    if (!record || !record->active || record->owner != binding->owner || !record->allocation)
        hl_fail(binding->owner, error_message);
    /* hl_ownership_drop detaches and invalidates but intentionally does not
     * execute callbacks. A binding replacement must clean its old resource
     * before cancelling the registration. */
    record->allocation->cleanup(record->allocation->context);
    if (!hl_ownership_drop(binding->owner, token))
        hl_fail(binding->owner, error_message);
}

static HlValue hl_store_value(HlBinding *binding, HlValue value, HlStorageClass storage) {
    if (hl_is_aggregate_tag(value.tag)) {
        if (binding->value.tag == HL_STRING && binding->ownership_token) {
            hl_release_binding_resource(binding, "no se pudo liberar la propiedad anterior de cadena");
        }

        HlValue stored;
        if (storage == HL_STORAGE_OWNED) {
            if (hl_is_aggregate_tag(binding->value.tag) && binding->ownership_token) {
                binding->value.tag = HL_NUMBER;
                binding->value.as.number = 0.0;
                hl_lane_destroy(&binding->aggregate_lane);
                hl_lane_init(&binding->aggregate_lane);
            } else {
                hl_release_binding_resource(binding, "no se pudo liberar la propiedad anterior del binding");
                hl_lane_init(&binding->aggregate_lane);
                if (!hl_ownership_acquire(binding->owner, binding->ownership_site,
                        &binding->ownership_token, hl_cleanup_binding_aggregate, binding))
                    hl_fail(binding->owner, "no se pudo adquirir la propiedad de un array");
            }
            stored = hl_clone_value_in_lane(binding->owner, &binding->aggregate_lane, value);
        } else {
            hl_release_binding_resource(binding, "no se pudo liberar la propiedad anterior del binding");
            stored = hl_clone_value(binding->owner, value);
        }
        hl_heap_free(binding->string_storage);
        binding->string_storage = NULL;
        binding->string_capacity = 0;
        return stored;
    }

    if (value.tag == HL_STRING) {
        if (hl_is_aggregate_tag(binding->value.tag) && binding->ownership_token) {
            hl_release_binding_resource(binding, "no se pudo liberar la propiedad anterior de array");
        }

        /* ScopeRegion strings are copied into the lexical owner's arena. */
        if (storage == HL_STORAGE_SCOPE) {
            hl_release_binding_resource(binding, "no se pudo liberar la propiedad anterior de cadena");
            char *scoped = hl_copy_string_n(binding->owner, value.as.string, value.string_length);
            hl_heap_free(binding->string_storage);
            binding->string_storage = NULL;
            binding->string_capacity = 0;
            value.as.string = scoped;
            return value;
        }

        if (!binding->ownership_token) {
            if (!hl_ownership_acquire(binding->owner, binding->ownership_site,
                    &binding->ownership_token, hl_cleanup_binding_string, binding))
                hl_fail(binding->owner, "no se pudo adquirir la propiedad de una cadena");
            HlOwnershipRecord *record = hl_ownership_find(binding->owner, binding->ownership_token);
            if (!record || !record->allocation)
                hl_fail(binding->owner, "registro de propiedad de cadena inconsistente");
        }

        if (value.string_length == SIZE_MAX)
            hl_fail(binding->owner, "cadena demasiado grande al almacenar");
        const size_t size = value.string_length + 1;
        char *incoming = (char *)hl_heap_alloc(binding->owner, size);
        if (!incoming) hl_fail(binding->owner, "memoria agotada al copiar una cadena");
        memcpy(incoming, value.as.string, size);

        if (size > binding->string_capacity) {
            char *grown = (char *)hl_heap_realloc(binding->owner, binding->string_storage, size);
            if (!grown) {
                hl_heap_free(incoming);
                hl_fail(binding->owner, "memoria agotada al ampliar una cadena");
            }
            binding->string_storage = grown;
            binding->string_capacity = size;
        }
        memcpy(binding->string_storage, incoming, size);
        hl_heap_free(incoming);
        value.as.string = binding->string_storage;
        return value;
    }

    hl_release_binding_resource(binding, "no se pudo liberar la propiedad anterior del binding");
    hl_heap_free(binding->string_storage);
    binding->string_storage = NULL;
    binding->string_capacity = 0;
    return value;
}
static void hl_define_planned_site(HlEnv *environment, const char *name, HlValue value,
        HlStorageClass storage, uint64_t ownership_site) {
    HlBinding *item = hl_find_local(environment, name);
    if (item) {
        item->value = hl_store_value(item, value, storage);
        if (item->cell) hl_cell_write(item->cell, item->value);
        return;
    }
    item = (HlBinding *)hl_alloc(environment, sizeof(HlBinding));
    item->name = hl_copy_string(environment, name);
    item->owner = environment;
    item->string_storage = NULL;
    item->string_capacity = 0;
    item->ownership_site = ownership_site;
    item->ownership_token = 0;
    item->cell = NULL;
    item->aggregate_lane = (HlArenaLane){0};
    item->value.tag = HL_NUMBER;
    item->value.as.number = 0.0;
    item->value = hl_store_value(item, value, storage);
    item->next = environment->bindings;
    environment->bindings = item;
}
static void hl_define_planned(HlEnv *environment, const char *name, HlValue value, HlStorageClass storage) {
    hl_define_planned_site(environment, name, value, storage, 0);
}
static void hl_assign_planned_site(HlEnv *environment, const char *name, HlValue value,
        HlStorageClass storage, uint64_t ownership_site) {
    HlBinding *item = hl_find(environment, name);
    if (!item) {
        char message[512];
        snprintf(message, sizeof(message), "Variable '%s' no definida para asignación", name);
        hl_fail(environment, message);
    }
    if (ownership_site && item->ownership_site != ownership_site)
        hl_fail(environment, "site de propiedad incoherente en asignación planificada");
    item->value = hl_store_value(item, value, storage);
    if (item->cell) hl_cell_write(item->cell, item->value);
}
static void hl_assign_planned(HlEnv *environment, const char *name, HlValue value, HlStorageClass storage) {
    hl_assign_planned_site(environment, name, value, storage, 0);
}
/* Compatibility defaults for runtime-only fixtures that do not carry HIR. */
static void hl_define(HlEnv *environment, const char *name, HlValue value) {
    hl_define_planned(environment, name, value, HL_STORAGE_OWNED);
}
static void hl_assign(HlEnv *environment, const char *name, HlValue value) {
    hl_assign_planned(environment, name, value, HL_STORAGE_OWNED);
}
static HlValue hl_get(HlEnv *environment, const char *name) {
    HlBinding *item = hl_find(environment, name);
    if (!item) {
        char message[512];
        snprintf(message, sizeof(message), "Variable '%s' no definida", name);
        hl_fail(environment, message);
    }
    if (item->cell) {
        HlValue captured;
        if (!hl_cell_read(item->cell, &captured)) hl_fail(environment, "lectura de captura no inicializada");
        return captured.tag == HL_ARRAY || captured.tag == HL_MAP ? hl_clone_value(environment, captured) : captured;
    }
    /* Arrays have value semantics: each read receives an independent
     * graph allocated in the current activation's lane. */
    if (item->value.tag == HL_ARRAY || item->value.tag == HL_MAP)
        return hl_clone_value(environment, item->value);
    return item->value;
}
/* Array indexing follows the interpreter's integer, non-negative
 * index contract. Reads clone the selected value into the current lane. */
static size_t hl_array_index(HlEnv *environment, HlValue index) {
    if (index.tag != HL_NUMBER || !isfinite(index.as.number)
        || index.as.number < 0.0 || floor(index.as.number) != index.as.number)
        hl_fail(environment, "El índice de un array debe ser entero no negativo");
    /* A double cannot represent SIZE_MAX exactly on common 64-bit targets:
     * (double)SIZE_MAX rounds up to 2^N. Reject that boundary before the
     * floating-to-integer conversion, which would otherwise be undefined. */
    if (index.as.number >= (double)SIZE_MAX) return SIZE_MAX;
    return (size_t)index.as.number;
}
/* Strings are indexed by Unicode scalar value, matching Rust str::chars(). */
static size_t hl_string_index(HlEnv *environment, HlValue index) {
    if (index.tag != HL_NUMBER || !isfinite(index.as.number)
        || index.as.number < 0.0 || floor(index.as.number) != index.as.number)
        hl_fail(environment, "El índice de una cadena debe ser entero no negativo");
    if (index.as.number >= (double)SIZE_MAX) return SIZE_MAX;
    return (size_t)index.as.number;
}
static HlValue hl_string_at(HlEnv *environment, const char *value, size_t length, size_t position) {
    const unsigned char *bytes = (const unsigned char *)value;
    size_t offset = 0;
    size_t current = 0;
    while (offset < length && current < position) {
        unsigned char lead = bytes[offset];
        size_t width = (lead < 0x80u) ? 1u
            : ((lead & 0xE0u) == 0xC0u) ? 2u
            : ((lead & 0xF0u) == 0xE0u) ? 3u
            : ((lead & 0xF8u) == 0xF0u) ? 4u : 1u;
        if (width > length - offset) width = 1u;
        offset += width;
        ++current;
    }
    if (offset >= length || current != position)
        hl_fail(environment, "índice de cadena fuera de rango");
    unsigned char lead = bytes[offset];
    size_t width = (lead < 0x80u) ? 1u
        : ((lead & 0xE0u) == 0xC0u) ? 2u
        : ((lead & 0xF0u) == 0xE0u) ? 3u
        : ((lead & 0xF8u) == 0xF0u) ? 4u : 1u;
    if (width > length - offset) width = 1u;
    char character[5] = {0, 0, 0, 0, 0};
    memcpy(character, bytes + offset, width);
    return hl_string(environment, character, width);
}
static HlValue hl_index(HlEnv *environment, HlValue target, HlValue index) {
    if (target.tag == HL_ARRAY && target.as.array) {
        size_t position = hl_array_index(environment, index);
        if (position >= target.as.array->length)
            hl_fail(environment, "índice de array fuera de rango");
        return hl_clone_value(environment, target.as.array->items[position]);
    }
    if (target.tag == HL_MAP && target.as.map) {
        size_t position = hl_map_find_key(target.as.map, index);
        if (position == SIZE_MAX) hl_fail(environment, "Clave no encontrada en mapa");
        return hl_clone_value(environment, target.as.map->entries[position].value);
    }
    if (target.tag == HL_STRING && target.as.string) {
        size_t position = hl_string_index(environment, index);
        return hl_string_at(environment, target.as.string, target.string_length, position);
    }
    hl_fail(environment, "no se puede indexar el valor indicado");
    return hl_number(0);
}
static HlValue hl_clone_for_binding(HlBinding *binding, HlValue value) {
    return binding->ownership_token
        ? hl_clone_value_in_lane(binding->owner, &binding->aggregate_lane, value)
        : hl_clone_value(binding->owner, value);
}
static void hl_map_set(HlBinding *binding, HlMap *map, HlValue key, HlValue value) {
    size_t position = hl_map_find_key(map, key);
    HlValue stored_value = hl_clone_for_binding(binding, value);
    if (position != SIZE_MAX) {
        map->entries[position].value = stored_value;
        return;
    }
    HlValue stored_key = hl_clone_for_binding(binding, key);
    if (map->length >= SIZE_MAX / sizeof(HlMapEntry))
        hl_fail(binding->owner, "mapa demasiado grande al insertar clave");
    HlMapEntry *grown = (HlMapEntry *)hl_alloc_in_lane(binding->owner,
        binding->ownership_token ? &binding->aggregate_lane : binding->owner->lane,
        (map->length + 1) * sizeof(HlMapEntry));
    if (map->length) memcpy(grown, map->entries, map->length * sizeof(HlMapEntry));
    grown[map->length].key = stored_key;
    grown[map->length].value = stored_value;
    map->entries = grown;
    map->length += 1;
}
static HlValue hl_length(HlEnv *environment, HlValue value) {
    if (value.tag == HL_ARRAY && value.as.array)
        return hl_number((double)value.as.array->length);
    if (value.tag == HL_MAP && value.as.map)
        return hl_number((double)value.as.map->length);
    if (value.tag == HL_STRING && value.as.string) {
        size_t characters = 0;
        const unsigned char *bytes = (const unsigned char *)value.as.string;
        for (size_t i = 0; i < value.string_length; ++i) {
            if ((bytes[i] & 0xC0u) != 0x80u) ++characters;
        }
        return hl_number((double)characters);
    }
    hl_fail(environment, "len() requiere string, array o map");
    return hl_number(0);
}
/* A captured binding keeps its canonical value in a root-owned cell. Index
 * mutation happens in place on the binding value, so the shared cell must be
 * refreshed before the binding goes out of lexical scope. */
static void hl_binding_sync_cell(HlBinding *binding) {
    if (binding && binding->cell) hl_cell_write(binding->cell, binding->value);
}
static void hl_index_assign_path(HlEnv *environment, const char *name,
                                 const HlValue *indices, size_t index_count,
                                 HlValue value) {
    HlBinding *binding = hl_find(environment, name);
    if (!binding) {
        char message[512];
        snprintf(message, sizeof(message), "Variable '%s' no definida para asignación", name);
        hl_fail(environment, message);
    }
    if (!indices || index_count == 0)
        hl_fail(environment, "la asignación por índice requiere al menos un índice");

    /* A binding captured by a closure may have been updated through a sibling
     * closure since this alias was installed. Refresh from the canonical cell
     * before mutating, but keep the working copy in this binding's own lane;
     * hl_binding_sync_cell promotes the completed graph back to the cell. */
    if (binding->cell)
        binding->value = hl_clone_value(binding->owner, binding->cell->value);

    HlValue *slot = &binding->value;
    for (size_t depth = 0; depth < index_count; ++depth) {
        const int is_last = depth + 1 == index_count;
        if (slot->tag == HL_ARRAY && slot->as.array) {
            size_t position = hl_array_index(environment, indices[depth]);
            if (position >= slot->as.array->length)
                hl_fail(environment, "índice de array fuera de rango");
            slot = &slot->as.array->items[position];
            if (is_last) *slot = hl_clone_for_binding(binding, value);
            continue;
        }
        if (slot->tag == HL_MAP && slot->as.map) {
            size_t position = hl_map_find_key(slot->as.map, indices[depth]);
            if (is_last) {
                hl_map_set(binding, slot->as.map, indices[depth], value);
                hl_binding_sync_cell(binding);
                return;
            }
            if (position == SIZE_MAX) hl_fail(environment, "Clave no encontrada en mapa");
            slot = &slot->as.map->entries[position].value;
            continue;
        }
        hl_fail(environment, "no se puede indexar por asignación un valor que no sea colección");
    }
    hl_binding_sync_cell(binding);
}

/* Index assignment on activation storage (parameters, locals and temporaries
 * held in frame slots). The updated value is returned so the caller writes it
 * back into its slot; new storage belongs to the current environment lane. */
static HlValue hl_index_assign_value(HlEnv *environment, HlValue target,
                                     const HlValue *indices, size_t index_count,
                                     HlValue value) {
    if (!indices || index_count == 0)
        hl_fail(environment, "la asignación por índice requiere al menos un índice");
    HlValue *slot = &target;
    for (size_t depth = 0; depth < index_count; ++depth) {
        const int is_last = depth + 1 == index_count;
        if (slot->tag == HL_ARRAY && slot->as.array) {
            size_t position = hl_array_index(environment, indices[depth]);
            if (position >= slot->as.array->length)
                hl_fail(environment, "índice de array fuera de rango");
            slot = &slot->as.array->items[position];
            if (is_last) *slot = hl_clone_value(environment, value);
            continue;
        }
        if (slot->tag == HL_MAP && slot->as.map) {
            size_t position = hl_map_find_key(slot->as.map, indices[depth]);
            if (is_last) {
                HlValue stored = hl_clone_value(environment, value);
                if (position != SIZE_MAX) {
                    slot->as.map->entries[position].value = stored;
                } else {
                    size_t length = slot->as.map->length;
                    if (length >= SIZE_MAX / sizeof(HlMapEntry))
                        hl_fail(environment, "mapa demasiado grande al insertar clave");
                    HlMapEntry *grown = (HlMapEntry *)hl_alloc(environment,
                        (length + 1) * sizeof(HlMapEntry));
                    if (length) memcpy(grown, slot->as.map->entries, length * sizeof(HlMapEntry));
                    grown[length].key = hl_clone_value(environment, indices[depth]);
                    grown[length].value = stored;
                    slot->as.map->entries = grown;
                    slot->as.map->length = length + 1;
                }
                return target;
            }
            if (position == SIZE_MAX) hl_fail(environment, "Clave no encontrada en mapa");
            slot = &slot->as.map->entries[position].value;
            continue;
        }
        hl_fail(environment, "no se puede indexar por asignación un valor que no sea colección");
    }
    return target;
}

static void hl_index_assign(HlEnv *environment, const char *name, HlValue index, HlValue value) {
    hl_index_assign_path(environment, name, &index, 1, value);
}

static int hl_truthy(HlValue value) {
    switch (value.tag) {
        case HL_BOOL: return value.as.boolean;
        case HL_NUMBER: return value.as.number != 0.0;
        case HL_STRING: return value.string_length != 0;
        case HL_ARRAY: return value.as.array != NULL;
        case HL_MAP: return value.as.map != NULL;
        case HL_CLOSURE: return value.as.closure != NULL;
    }
    return 0;
}

/* Produce Rust Display-compatible fixed decimal text from a shortest round-trip value. */
static void hl_append_char(char *buffer, size_t size, size_t *length, char value) {
    if (*length + 1 < size) buffer[(*length)++] = value;
}
static void hl_number_text(double value, char *buffer, size_t size) {
    if (isnan(value)) { snprintf(buffer, size, "NaN"); return; }
    if (isinf(value)) { snprintf(buffer, size, "%sinf", signbit(value) ? "-" : ""); return; }

    char shortest[64];
    for (int precision = 1; precision <= 17; ++precision) {
        snprintf(shortest, sizeof(shortest), "%.*g", precision, value);
        if (strtod(shortest, NULL) == value) break;
    }

    char *exponent_mark = strchr(shortest, 'e');
    if (!exponent_mark) exponent_mark = strchr(shortest, 'E');
    if (!exponent_mark) { snprintf(buffer, size, "%s", shortest); return; }

    int exponent = (int)strtol(exponent_mark + 1, NULL, 10);
    char digits[32];
    size_t digit_count = 0;
    int decimal_position = 0;
    int after_decimal = 0;
    for (const char *cursor = shortest; cursor < exponent_mark; ++cursor) {
        if (*cursor == '.') { after_decimal = 1; continue; }
        if (*cursor >= '0' && *cursor <= '9') {
            if (!after_decimal) ++decimal_position;
            if (digit_count < sizeof(digits)) digits[digit_count++] = *cursor;
        }
    }
    decimal_position += exponent;

    size_t length = 0;
    const char *sign = shortest[0] == '-' ? "-" : "";
    if (*sign) hl_append_char(buffer, size, &length, '-');

    if (decimal_position <= 0) {
        hl_append_char(buffer, size, &length, '0');
        hl_append_char(buffer, size, &length, '.');
        for (int i = 0; i < -decimal_position; ++i)
            hl_append_char(buffer, size, &length, '0');
        for (size_t i = 0; i < digit_count; ++i)
            hl_append_char(buffer, size, &length, digits[i]);
    } else if ((size_t)decimal_position >= digit_count) {
        for (size_t i = 0; i < digit_count; ++i)
            hl_append_char(buffer, size, &length, digits[i]);
        for (size_t i = digit_count; i < (size_t)decimal_position; ++i)
            hl_append_char(buffer, size, &length, '0');
    } else {
        for (int i = 0; i < decimal_position; ++i)
            hl_append_char(buffer, size, &length, digits[i]);
        hl_append_char(buffer, size, &length, '.');
        for (size_t i = (size_t)decimal_position; i < digit_count; ++i)
            hl_append_char(buffer, size, &length, digits[i]);
    }
    if (size > 0) buffer[length < size ? length : size - 1] = '\0';
}
static const char *hl_to_text(HlEnv *environment, HlValue value) {
    if (value.tag == HL_STRING) return value.as.string;
    if (value.tag == HL_BOOL) return value.as.boolean ? "true" : "false";
    char buffer[512];
    hl_number_text(value.as.number, buffer, sizeof(buffer));
    return hl_copy_string(environment, buffer);
}
static void hl_debug_string(const char *value, size_t length) {
    putchar('"');
    for (size_t i = 0; value && i < length; ++i) {
        unsigned char byte = (unsigned char)value[i];
        switch (byte) {
            case 0: fputs("\\0", stdout); break;
            case '"': fputs("\\\"", stdout); break;
            case '\\': fputs("\\\\", stdout); break;
            case '\n': fputs("\\n", stdout); break;
            case '\r': fputs("\\r", stdout); break;
            case '\t': fputs("\\t", stdout); break;
            default:
                if (byte < 0x20) printf("\\u{%x}", (unsigned)byte);
                else putchar(byte);
        }
    }
    putchar('"');
}
static void hl_debug_value(HlValue value) {
    switch (value.tag) {
        case HL_NUMBER: {
            char buffer[512];
            hl_number_text(value.as.number, buffer, sizeof(buffer));
            if (isfinite(value.as.number) && !strchr(buffer, '.') && !strchr(buffer, 'e') && !strchr(buffer, 'E'))
                strncat(buffer, ".0", sizeof(buffer) - strlen(buffer) - 1);
            printf("Number(%s)", buffer);
            break;
        }
        case HL_BOOL: printf("Bool(%s)", value.as.boolean ? "true" : "false"); break;
        case HL_STRING: fputs("String(", stdout); hl_debug_string(value.as.string ? value.as.string : "", value.string_length); putchar(')'); break;
        case HL_ARRAY:
            fputs("Array([", stdout);
            if (value.as.array) for (size_t i = 0; i < value.as.array->length; ++i) {
                if (i) fputs(", ", stdout);
                hl_debug_value(value.as.array->items[i]);
            }
            fputs("])", stdout);
            break;
        case HL_CLOSURE: printf("Closure(%u)", value.as.closure ? value.as.closure->function_id : 0); break;
        case HL_MAP:
            fputs("Map([", stdout);
            if (value.as.map) for (size_t i = 0; i < value.as.map->length; ++i) {
                if (i) fputs(", ", stdout);
                putchar('(');
                hl_debug_value(value.as.map->entries[i].key);
                fputs(", ", stdout);
                hl_debug_value(value.as.map->entries[i].value);
                putchar(')');
            }
            fputs("])", stdout);
            break;
    }
}
static void hl_debug_print_array(HlValue value) {
    putchar('[');
    if (value.as.array) for (size_t i = 0; i < value.as.array->length; ++i) {
        if (i) fputs(", ", stdout);
        hl_debug_value(value.as.array->items[i]);
    }
    putchar(']');
}
static void hl_debug_print_map(HlValue value) {
    putchar('[');
    if (value.as.map) for (size_t i = 0; i < value.as.map->length; ++i) {
        if (i) fputs(", ", stdout);
        putchar('(');
        hl_debug_value(value.as.map->entries[i].key);
        fputs(", ", stdout);
        hl_debug_value(value.as.map->entries[i].value);
        putchar(')');
    }
    putchar(']');
}
static void hl_print(HlValue value) {
    switch (value.tag) {
        case HL_NUMBER: {
            char buffer[512];
            hl_number_text(value.as.number, buffer, sizeof(buffer));
            puts(buffer);
            break;
        }
        case HL_BOOL: puts(value.as.boolean ? "true" : "false"); break;
        case HL_STRING:
            if (value.as.string && value.string_length)
                fwrite(value.as.string, 1, value.string_length, stdout);
            putchar('\n');
            break;
        case HL_ARRAY: hl_debug_print_array(value); putchar('\n'); break;
        case HL_MAP: hl_debug_print_map(value); putchar('\n'); break;
        case HL_CLOSURE: printf("<closure:%u>\\n", value.as.closure ? value.as.closure->function_id : 0); break;
    }
}
static int hl_is_string_number_pair(HlValue left, HlValue right) {
    return (left.tag == HL_STRING && right.tag == HL_NUMBER)
        || (left.tag == HL_NUMBER && right.tag == HL_STRING);
}
static size_t hl_text_length(HlValue value, const char *text) {
    return value.tag == HL_STRING ? value.string_length : strlen(text);
}
static HlValue hl_mixed_text(HlEnv *environment, HlValue left, HlValue right) {
    const char *left_text = hl_to_text(environment, left);
    const char *right_text = hl_to_text(environment, right);
    size_t left_length = hl_text_length(left, left_text);
    size_t right_length = hl_text_length(right, right_text);
    if (left_length > SIZE_MAX - 2 || right_length > SIZE_MAX - left_length - 2)
        hl_fail(environment, "cadena demasiado grande");
    size_t result_length = left_length + right_length + 2;
    char *joined = (char *)hl_alloc(environment, result_length + 1);
    memcpy(joined, left_text, left_length);
    joined[left_length] = '(';
    memcpy(joined + left_length + 1, right_text, right_length);
    joined[left_length + right_length + 1] = ')';
    joined[result_length] = '\0';
    return hl_string(environment, joined, result_length);
}
static void hl_require_numbers(HlEnv *environment, HlValue left, HlValue right, const char *operation) {
    if (left.tag != HL_NUMBER || right.tag != HL_NUMBER) {
        char message[256];
        snprintf(message, sizeof(message), "Type mismatch: valores incompatibles con el operador %s", operation);
        hl_fail(environment, message);
    }
}
static HlValue hl_add(HlEnv *environment, HlValue left, HlValue right) {
    if (left.tag == HL_NUMBER && right.tag == HL_NUMBER)
        return hl_number(left.as.number + right.as.number);
    if (left.tag == HL_STRING && right.tag == HL_NUMBER) {
        const char *number = hl_to_text(environment, right);
        size_t number_length = strlen(number);
        if (left.string_length > SIZE_MAX - number_length - 1)
            hl_fail(environment, "cadena demasiado grande");
        size_t result_length = left.string_length + number_length;
        char *joined = (char *)hl_alloc(environment, result_length + 1);
        memcpy(joined, left.as.string, left.string_length);
        memcpy(joined + left.string_length, number, number_length + 1);
        return hl_string(environment, joined, result_length);
    }
    if (left.tag == HL_NUMBER && right.tag == HL_STRING) {
        const char *number = hl_to_text(environment, left);
        size_t number_length = strlen(number);
        if (number_length > SIZE_MAX - right.string_length - 1)
            hl_fail(environment, "cadena demasiado grande");
        size_t result_length = number_length + right.string_length;
        char *joined = (char *)hl_alloc(environment, result_length + 1);
        memcpy(joined, number, number_length);
        memcpy(joined + number_length, right.as.string, right.string_length);
        joined[result_length] = '\0';
        return hl_string(environment, joined, result_length);
    }
    hl_fail(environment, "Type mismatch: valores incompatibles con el operador Plus");
    return hl_number(0);
}
static HlValue hl_sub(HlEnv *environment, HlValue left, HlValue right) {
    if (hl_is_string_number_pair(left, right)) return hl_mixed_text(environment, left, right);
    hl_require_numbers(environment, left, right, "Minus"); return hl_number(left.as.number - right.as.number);
}
static HlValue hl_mul(HlEnv *environment, HlValue left, HlValue right) {
    if (hl_is_string_number_pair(left, right)) return hl_mixed_text(environment, left, right);
    hl_require_numbers(environment, left, right, "Star"); return hl_number(left.as.number * right.as.number);
}
static HlValue hl_div(HlEnv *environment, HlValue left, HlValue right) {
    if (hl_is_string_number_pair(left, right)) return hl_mixed_text(environment, left, right);
    hl_require_numbers(environment, left, right, "Slash");
    if (right.as.number == 0.0) hl_fail(environment, "Division by zero");
    return hl_number(left.as.number / right.as.number);
}
static HlValue hl_less(HlEnv *environment, HlValue left, HlValue right) {
    if (hl_is_string_number_pair(left, right)) return hl_mixed_text(environment, left, right);
    hl_require_numbers(environment, left, right, "Less"); return hl_bool(left.as.number < right.as.number);
}
static HlValue hl_greater(HlEnv *environment, HlValue left, HlValue right) {
    if (hl_is_string_number_pair(left, right)) return hl_mixed_text(environment, left, right);
    hl_require_numbers(environment, left, right, "Greater"); return hl_bool(left.as.number > right.as.number);
}
static HlValue hl_equal(HlEnv *environment, HlValue left, HlValue right) {
    if (hl_is_string_number_pair(left, right)) return hl_mixed_text(environment, left, right);
    hl_require_numbers(environment, left, right, "Tilde"); return hl_bool(left.as.number == right.as.number);
}
static HlValue hl_not_equal(HlEnv *environment, HlValue left, HlValue right) {
    if (hl_is_string_number_pair(left, right)) return hl_mixed_text(environment, left, right);
    hl_require_numbers(environment, left, right, "Caret");
    return hl_bool(fabs(left.as.number - right.as.number) > DBL_EPSILON);
}
