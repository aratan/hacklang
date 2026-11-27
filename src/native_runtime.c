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
static HlValue hl_string_n(HlEnv *environment, const char *value, size_t length) {
    HlValue result = {0};
    result.tag = HL_STRING;
    result.as.string = hl_copy_string_n(environment, value, length);
    result.string_length = length;
    return result;
}
/* Keep the legacy C helper for runtime unit fixtures and ordinary C strings. */
static HlValue hl_string(HlEnv *environment, const char *value) {
    return hl_string_n(environment, value, strlen(value));
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
        case HL_STRING: return hl_string_n(environment, value.as.string, value.string_length);
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
    return hl_string_n(environment, character, width);
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
static unsigned int hl_debug_codepoint(const unsigned char *bytes, size_t length, size_t *width) {
    unsigned char lead = bytes[0];
    if (lead < 0x80u) { *width = 1; return lead; }
    size_t count = (lead & 0xE0u) == 0xC0u ? 2u
        : (lead & 0xF0u) == 0xE0u ? 3u
        : (lead & 0xF8u) == 0xF0u ? 4u : 1u;
    if (count == 1u || count > length) { *width = 1; return lead; }
    unsigned int codepoint = lead & (count == 2u ? 0x1Fu : count == 3u ? 0x0Fu : 0x07u);
    for (size_t i = 1; i < count; ++i) {
        unsigned char continuation = bytes[i];
        if ((continuation & 0xC0u) != 0x80u) { *width = 1; return lead; }
        codepoint = (codepoint << 6) | (unsigned int)(continuation & 0x3Fu);
    }
    *width = count;
    return codepoint;
}
typedef struct { unsigned int first; unsigned int last; } HlUnicodeRange;
static const HlUnicodeRange hl_nonprinting_ranges[] = {
    {0x0u, 0x1fu},
    {0x7fu, 0x9fu},
    {0xadu, 0xadu},
    {0x300u, 0x36fu},
    {0x378u, 0x379u},
    {0x380u, 0x383u},
    {0x38bu, 0x38bu},
    {0x38du, 0x38du},
    {0x3a2u, 0x3a2u},
    {0x483u, 0x489u},
    {0x530u, 0x530u},
    {0x557u, 0x558u},
    {0x58bu, 0x58cu},
    {0x590u, 0x5bdu},
    {0x5bfu, 0x5bfu},
    {0x5c1u, 0x5c2u},
    {0x5c4u, 0x5c5u},
    {0x5c7u, 0x5cfu},
    {0x5ebu, 0x5eeu},
    {0x5f5u, 0x605u},
    {0x610u, 0x61au},
    {0x61cu, 0x61cu},
    {0x64bu, 0x65fu},
    {0x670u, 0x670u},
    {0x6d6u, 0x6ddu},
    {0x6dfu, 0x6e4u},
    {0x6e7u, 0x6e8u},
    {0x6eau, 0x6edu},
    {0x70eu, 0x70fu},
    {0x711u, 0x711u},
    {0x730u, 0x74cu},
    {0x7a6u, 0x7b0u},
    {0x7b2u, 0x7bfu},
    {0x7ebu, 0x7f3u},
    {0x7fbu, 0x7fdu},
    {0x816u, 0x819u},
    {0x81bu, 0x823u},
    {0x825u, 0x827u},
    {0x829u, 0x82fu},
    {0x83fu, 0x83fu},
    {0x859u, 0x85du},
    {0x85fu, 0x85fu},
    {0x86bu, 0x86fu},
    {0x890u, 0x89fu},
    {0x8cau, 0x902u},
    {0x93au, 0x93au},
    {0x93cu, 0x93cu},
    {0x941u, 0x948u},
    {0x94du, 0x94du},
    {0x951u, 0x957u},
    {0x962u, 0x963u},
    {0x981u, 0x981u},
    {0x984u, 0x984u},
    {0x98du, 0x98eu},
    {0x991u, 0x992u},
    {0x9a9u, 0x9a9u},
    {0x9b1u, 0x9b1u},
    {0x9b3u, 0x9b5u},
    {0x9bau, 0x9bcu},
    {0x9c1u, 0x9c6u},
    {0x9c9u, 0x9cau},
    {0x9cdu, 0x9cdu},
    {0x9cfu, 0x9d6u},
    {0x9d8u, 0x9dbu},
    {0x9deu, 0x9deu},
    {0x9e2u, 0x9e5u},
    {0x9feu, 0xa02u},
    {0xa04u, 0xa04u},
    {0xa0bu, 0xa0eu},
    {0xa11u, 0xa12u},
    {0xa29u, 0xa29u},
    {0xa31u, 0xa31u},
    {0xa34u, 0xa34u},
    {0xa37u, 0xa37u},
    {0xa3au, 0xa3du},
    {0xa41u, 0xa58u},
    {0xa5du, 0xa5du},
    {0xa5fu, 0xa65u},
    {0xa70u, 0xa71u},
    {0xa75u, 0xa75u},
    {0xa77u, 0xa82u},
    {0xa84u, 0xa84u},
    {0xa8eu, 0xa8eu},
    {0xa92u, 0xa92u},
    {0xaa9u, 0xaa9u},
    {0xab1u, 0xab1u},
    {0xab4u, 0xab4u},
    {0xabau, 0xabcu},
    {0xac1u, 0xac8u},
    {0xacau, 0xacau},
    {0xacdu, 0xacfu},
    {0xad1u, 0xadfu},
    {0xae2u, 0xae5u},
    {0xaf2u, 0xaf8u},
    {0xafau, 0xb01u},
    {0xb04u, 0xb04u},
    {0xb0du, 0xb0eu},
    {0xb11u, 0xb12u},
    {0xb29u, 0xb29u},
    {0xb31u, 0xb31u},
    {0xb34u, 0xb34u},
    {0xb3au, 0xb3cu},
    {0xb3fu, 0xb3fu},
    {0xb41u, 0xb46u},
    {0xb49u, 0xb4au},
    {0xb4du, 0xb56u},
    {0xb58u, 0xb5bu},
    {0xb5eu, 0xb5eu},
    {0xb62u, 0xb65u},
    {0xb78u, 0xb82u},
    {0xb84u, 0xb84u},
    {0xb8bu, 0xb8du},
    {0xb91u, 0xb91u},
    {0xb96u, 0xb98u},
    {0xb9bu, 0xb9bu},
    {0xb9du, 0xb9du},
    {0xba0u, 0xba2u},
    {0xba5u, 0xba7u},
    {0xbabu, 0xbadu},
    {0xbbau, 0xbbdu},
    {0xbc0u, 0xbc0u},
    {0xbc3u, 0xbc5u},
    {0xbc9u, 0xbc9u},
    {0xbcdu, 0xbcfu},
    {0xbd1u, 0xbd6u},
    {0xbd8u, 0xbe5u},
    {0xbfbu, 0xc00u},
    {0xc04u, 0xc04u},
    {0xc0du, 0xc0du},
    {0xc11u, 0xc11u},
    {0xc29u, 0xc29u},
    {0xc3au, 0xc3cu},
    {0xc3eu, 0xc40u},
    {0xc45u, 0xc57u},
    {0xc5bu, 0xc5bu},
    {0xc5eu, 0xc5fu},
    {0xc62u, 0xc65u},
    {0xc70u, 0xc76u},
    {0xc81u, 0xc81u},
    {0xc8du, 0xc8du},
    {0xc91u, 0xc91u},
    {0xca9u, 0xca9u},
    {0xcb4u, 0xcb4u},
    {0xcbau, 0xcbcu},
    {0xcbfu, 0xcbfu},
    {0xcc5u, 0xcc6u},
    {0xcc9u, 0xcc9u},
    {0xcccu, 0xcd4u},
    {0xcd7u, 0xcdbu},
    {0xcdfu, 0xcdfu},
    {0xce2u, 0xce5u},
    {0xcf0u, 0xcf0u},
    {0xcf4u, 0xd01u},
    {0xd0du, 0xd0du},
    {0xd11u, 0xd11u},
    {0xd3bu, 0xd3cu},
    {0xd41u, 0xd45u},
    {0xd49u, 0xd49u},
    {0xd4du, 0xd4du},
    {0xd50u, 0xd53u},
    {0xd62u, 0xd65u},
    {0xd80u, 0xd81u},
    {0xd84u, 0xd84u},
    {0xd97u, 0xd99u},
    {0xdb2u, 0xdb2u},
    {0xdbcu, 0xdbcu},
    {0xdbeu, 0xdbfu},
    {0xdc7u, 0xdceu},
    {0xdd2u, 0xdd7u},
    {0xde0u, 0xde5u},
    {0xdf0u, 0xdf1u},
    {0xdf5u, 0xe00u},
    {0xe31u, 0xe31u},
    {0xe34u, 0xe3eu},
    {0xe47u, 0xe4eu},
    {0xe5cu, 0xe80u},
    {0xe83u, 0xe83u},
    {0xe85u, 0xe85u},
    {0xe8bu, 0xe8bu},
    {0xea4u, 0xea4u},
    {0xea6u, 0xea6u},
    {0xeb1u, 0xeb1u},
    {0xeb4u, 0xebcu},
    {0xebeu, 0xebfu},
    {0xec5u, 0xec5u},
    {0xec7u, 0xecfu},
    {0xedau, 0xedbu},
    {0xee0u, 0xeffu},
    {0xf18u, 0xf19u},
    {0xf35u, 0xf35u},
    {0xf37u, 0xf37u},
    {0xf39u, 0xf39u},
    {0xf48u, 0xf48u},
    {0xf6du, 0xf7eu},
    {0xf80u, 0xf84u},
    {0xf86u, 0xf87u},
    {0xf8du, 0xfbdu},
    {0xfc6u, 0xfc6u},
    {0xfcdu, 0xfcdu},
    {0xfdbu, 0xfffu},
    {0x102du, 0x1030u},
    {0x1032u, 0x1037u},
    {0x1039u, 0x103au},
    {0x103du, 0x103eu},
    {0x1058u, 0x1059u},
    {0x105eu, 0x1060u},
    {0x1071u, 0x1074u},
    {0x1082u, 0x1082u},
    {0x1085u, 0x1086u},
    {0x108du, 0x108du},
    {0x109du, 0x109du},
    {0x10c6u, 0x10c6u},
    {0x10c8u, 0x10ccu},
    {0x10ceu, 0x10cfu},
    {0x1249u, 0x1249u},
    {0x124eu, 0x124fu},
    {0x1257u, 0x1257u},
    {0x1259u, 0x1259u},
    {0x125eu, 0x125fu},
    {0x1289u, 0x1289u},
    {0x128eu, 0x128fu},
    {0x12b1u, 0x12b1u},
    {0x12b6u, 0x12b7u},
    {0x12bfu, 0x12bfu},
    {0x12c1u, 0x12c1u},
    {0x12c6u, 0x12c7u},
    {0x12d7u, 0x12d7u},
    {0x1311u, 0x1311u},
    {0x1316u, 0x1317u},
    {0x135bu, 0x135fu},
    {0x137du, 0x137fu},
    {0x139au, 0x139fu},
    {0x13f6u, 0x13f7u},
    {0x13feu, 0x13ffu},
    {0x169du, 0x169fu},
    {0x16f9u, 0x16ffu},
    {0x1712u, 0x1714u},
    {0x1716u, 0x171eu},
    {0x1732u, 0x1733u},
    {0x1737u, 0x173fu},
    {0x1752u, 0x175fu},
    {0x176du, 0x176du},
    {0x1771u, 0x177fu},
    {0x17b4u, 0x17b5u},
    {0x17b7u, 0x17bdu},
    {0x17c6u, 0x17c6u},
    {0x17c9u, 0x17d3u},
    {0x17ddu, 0x17dfu},
    {0x17eau, 0x17efu},
    {0x17fau, 0x17ffu},
    {0x180bu, 0x180fu},
    {0x181au, 0x181fu},
    {0x1879u, 0x187fu},
    {0x1885u, 0x1886u},
    {0x18a9u, 0x18a9u},
    {0x18abu, 0x18afu},
    {0x18f6u, 0x18ffu},
    {0x191fu, 0x1922u},
    {0x1927u, 0x1928u},
    {0x192cu, 0x192fu},
    {0x1932u, 0x1932u},
    {0x1939u, 0x193fu},
    {0x1941u, 0x1943u},
    {0x196eu, 0x196fu},
    {0x1975u, 0x197fu},
    {0x19acu, 0x19afu},
    {0x19cau, 0x19cfu},
    {0x19dbu, 0x19ddu},
    {0x1a17u, 0x1a18u},
    {0x1a1bu, 0x1a1du},
    {0x1a56u, 0x1a56u},
    {0x1a58u, 0x1a60u},
    {0x1a62u, 0x1a62u},
    {0x1a65u, 0x1a6cu},
    {0x1a73u, 0x1a7fu},
    {0x1a8au, 0x1a8fu},
    {0x1a9au, 0x1a9fu},
    {0x1aaeu, 0x1b03u},
    {0x1b34u, 0x1b34u},
    {0x1b36u, 0x1b3au},
    {0x1b3cu, 0x1b3cu},
    {0x1b42u, 0x1b42u},
    {0x1b4du, 0x1b4du},
    {0x1b6bu, 0x1b73u},
    {0x1b80u, 0x1b81u},
    {0x1ba2u, 0x1ba5u},
    {0x1ba8u, 0x1ba9u},
    {0x1babu, 0x1badu},
    {0x1be6u, 0x1be6u},
    {0x1be8u, 0x1be9u},
    {0x1bedu, 0x1bedu},
    {0x1befu, 0x1bf1u},
    {0x1bf4u, 0x1bfbu},
    {0x1c2cu, 0x1c33u},
    {0x1c36u, 0x1c3au},
    {0x1c4au, 0x1c4cu},
    {0x1c8bu, 0x1c8fu},
    {0x1cbbu, 0x1cbcu},
    {0x1cc8u, 0x1cd2u},
    {0x1cd4u, 0x1ce0u},
    {0x1ce2u, 0x1ce8u},
    {0x1cedu, 0x1cedu},
    {0x1cf4u, 0x1cf4u},
    {0x1cf8u, 0x1cf9u},
    {0x1cfbu, 0x1cffu},
    {0x1dc0u, 0x1dffu},
    {0x1f16u, 0x1f17u},
    {0x1f1eu, 0x1f1fu},
    {0x1f46u, 0x1f47u},
    {0x1f4eu, 0x1f4fu},
    {0x1f58u, 0x1f58u},
    {0x1f5au, 0x1f5au},
    {0x1f5cu, 0x1f5cu},
    {0x1f5eu, 0x1f5eu},
    {0x1f7eu, 0x1f7fu},
    {0x1fb5u, 0x1fb5u},
    {0x1fc5u, 0x1fc5u},
    {0x1fd4u, 0x1fd5u},
    {0x1fdcu, 0x1fdcu},
    {0x1ff0u, 0x1ff1u},
    {0x1ff5u, 0x1ff5u},
    {0x1fffu, 0x1fffu},
    {0x200bu, 0x200fu},
    {0x2028u, 0x202eu},
    {0x2060u, 0x206fu},
    {0x2072u, 0x2073u},
    {0x208fu, 0x208fu},
    {0x209du, 0x209fu},
    {0x20c2u, 0x20ffu},
    {0x218cu, 0x218fu},
    {0x242au, 0x243fu},
    {0x244bu, 0x245fu},
    {0x2b74u, 0x2b75u},
    {0x2cefu, 0x2cf1u},
    {0x2cf4u, 0x2cf8u},
    {0x2d26u, 0x2d26u},
    {0x2d28u, 0x2d2cu},
    {0x2d2eu, 0x2d2fu},
    {0x2d68u, 0x2d6eu},
    {0x2d71u, 0x2d7fu},
    {0x2d97u, 0x2d9fu},
    {0x2da7u, 0x2da7u},
    {0x2dafu, 0x2dafu},
    {0x2db7u, 0x2db7u},
    {0x2dbfu, 0x2dbfu},
    {0x2dc7u, 0x2dc7u},
    {0x2dcfu, 0x2dcfu},
    {0x2dd7u, 0x2dd7u},
    {0x2ddfu, 0x2dffu},
    {0x2e5eu, 0x2e7fu},
    {0x2e9au, 0x2e9au},
    {0x2ef4u, 0x2effu},
    {0x2fd6u, 0x2fefu},
    {0x302au, 0x302du},
    {0x3040u, 0x3040u},
    {0x3097u, 0x309au},
    {0x3100u, 0x3104u},
    {0x3130u, 0x3130u},
    {0x318fu, 0x318fu},
    {0x31e6u, 0x31eeu},
    {0x321fu, 0x321fu},
    {0xa48du, 0xa48fu},
    {0xa4c7u, 0xa4cfu},
    {0xa62cu, 0xa63fu},
    {0xa66fu, 0xa672u},
    {0xa674u, 0xa67du},
    {0xa69eu, 0xa69fu},
    {0xa6f0u, 0xa6f1u},
    {0xa6f8u, 0xa6ffu},
    {0xa7ddu, 0xa7f0u},
    {0xa802u, 0xa802u},
    {0xa806u, 0xa806u},
    {0xa80bu, 0xa80bu},
    {0xa825u, 0xa826u},
    {0xa82cu, 0xa82fu},
    {0xa83au, 0xa83fu},
    {0xa878u, 0xa87fu},
    {0xa8c4u, 0xa8cdu},
    {0xa8dau, 0xa8f1u},
    {0xa8ffu, 0xa8ffu},
    {0xa926u, 0xa92du},
    {0xa947u, 0xa951u},
    {0xa954u, 0xa95eu},
    {0xa97du, 0xa982u},
    {0xa9b3u, 0xa9b3u},
    {0xa9b6u, 0xa9b9u},
    {0xa9bcu, 0xa9bdu},
    {0xa9ceu, 0xa9ceu},
    {0xa9dau, 0xa9ddu},
    {0xa9e5u, 0xa9e5u},
    {0xa9ffu, 0xa9ffu},
    {0xaa29u, 0xaa2eu},
    {0xaa31u, 0xaa32u},
    {0xaa35u, 0xaa3fu},
    {0xaa43u, 0xaa43u},
    {0xaa4cu, 0xaa4cu},
    {0xaa4eu, 0xaa4fu},
    {0xaa5au, 0xaa5bu},
    {0xaa7cu, 0xaa7cu},
    {0xaab0u, 0xaab0u},
    {0xaab2u, 0xaab4u},
    {0xaab7u, 0xaab8u},
    {0xaabeu, 0xaabfu},
    {0xaac1u, 0xaac1u},
    {0xaac3u, 0xaadau},
    {0xaaecu, 0xaaedu},
    {0xaaf6u, 0xab00u},
    {0xab07u, 0xab08u},
    {0xab0fu, 0xab10u},
    {0xab17u, 0xab1fu},
    {0xab27u, 0xab27u},
    {0xab2fu, 0xab2fu},
    {0xab6cu, 0xab6fu},
    {0xabe5u, 0xabe5u},
    {0xabe8u, 0xabe8u},
    {0xabedu, 0xabefu},
    {0xabfau, 0xabffu},
    {0xd7a4u, 0xd7afu},
    {0xd7c7u, 0xd7cau},
    {0xd7fcu, 0xf8ffu},
    {0xfa6eu, 0xfa6fu},
    {0xfadau, 0xfaffu},
    {0xfb07u, 0xfb12u},
    {0xfb18u, 0xfb1cu},
    {0xfb1eu, 0xfb1eu},
    {0xfb37u, 0xfb37u},
    {0xfb3du, 0xfb3du},
    {0xfb3fu, 0xfb3fu},
    {0xfb42u, 0xfb42u},
    {0xfb45u, 0xfb45u},
    {0xfdd0u, 0xfdefu},
    {0xfe00u, 0xfe0fu},
    {0xfe1au, 0xfe2fu},
    {0xfe53u, 0xfe53u},
    {0xfe67u, 0xfe67u},
    {0xfe6cu, 0xfe6fu},
    {0xfe75u, 0xfe75u},
    {0xfefdu, 0xff00u},
    {0xffbfu, 0xffc1u},
    {0xffc8u, 0xffc9u},
    {0xffd0u, 0xffd1u},
    {0xffd8u, 0xffd9u},
    {0xffddu, 0xffdfu},
    {0xffe7u, 0xffe7u},
    {0xffefu, 0xfffbu},
    {0xfffeu, 0xffffu},
    {0x1000cu, 0x1000cu},
    {0x10027u, 0x10027u},
    {0x1003bu, 0x1003bu},
    {0x1003eu, 0x1003eu},
    {0x1004eu, 0x1004fu},
    {0x1005eu, 0x1007fu},
    {0x100fbu, 0x100ffu},
    {0x10103u, 0x10106u},
    {0x10134u, 0x10136u},
    {0x1018fu, 0x1018fu},
    {0x1019du, 0x1019fu},
    {0x101a1u, 0x101cfu},
    {0x101fdu, 0x1027fu},
    {0x1029du, 0x1029fu},
    {0x102d1u, 0x102e0u},
    {0x102fcu, 0x102ffu},
    {0x10324u, 0x1032cu},
    {0x1034bu, 0x1034fu},
    {0x10376u, 0x1037fu},
    {0x1039eu, 0x1039eu},
    {0x103c4u, 0x103c7u},
    {0x103d6u, 0x103ffu},
    {0x1049eu, 0x1049fu},
    {0x104aau, 0x104afu},
    {0x104d4u, 0x104d7u},
    {0x104fcu, 0x104ffu},
    {0x10528u, 0x1052fu},
    {0x10564u, 0x1056eu},
    {0x1057bu, 0x1057bu},
    {0x1058bu, 0x1058bu},
    {0x10593u, 0x10593u},
    {0x10596u, 0x10596u},
    {0x105a2u, 0x105a2u},
    {0x105b2u, 0x105b2u},
    {0x105bau, 0x105bau},
    {0x105bdu, 0x105bfu},
    {0x105f4u, 0x105ffu},
    {0x10737u, 0x1073fu},
    {0x10756u, 0x1075fu},
    {0x10768u, 0x1077fu},
    {0x10786u, 0x10786u},
    {0x107b1u, 0x107b1u},
    {0x107bbu, 0x107ffu},
    {0x10806u, 0x10807u},
    {0x10809u, 0x10809u},
    {0x10836u, 0x10836u},
    {0x10839u, 0x1083bu},
    {0x1083du, 0x1083eu},
    {0x10856u, 0x10856u},
    {0x1089fu, 0x108a6u},
    {0x108b0u, 0x108dfu},
    {0x108f3u, 0x108f3u},
    {0x108f6u, 0x108fau},
    {0x1091cu, 0x1091eu},
    {0x1093au, 0x1093eu},
    {0x1095au, 0x1097fu},
    {0x109b8u, 0x109bbu},
    {0x109d0u, 0x109d1u},
    {0x10a01u, 0x10a0fu},
    {0x10a14u, 0x10a14u},
    {0x10a18u, 0x10a18u},
    {0x10a36u, 0x10a3fu},
    {0x10a49u, 0x10a4fu},
    {0x10a59u, 0x10a5fu},
    {0x10aa0u, 0x10abfu},
    {0x10ae5u, 0x10aeau},
    {0x10af7u, 0x10affu},
    {0x10b36u, 0x10b38u},
    {0x10b56u, 0x10b57u},
    {0x10b73u, 0x10b77u},
    {0x10b92u, 0x10b98u},
    {0x10b9du, 0x10ba8u},
    {0x10bb0u, 0x10bffu},
    {0x10c49u, 0x10c7fu},
    {0x10cb3u, 0x10cbfu},
    {0x10cf3u, 0x10cf9u},
    {0x10d24u, 0x10d2fu},
    {0x10d3au, 0x10d3fu},
    {0x10d66u, 0x10d6du},
    {0x10d86u, 0x10d8du},
    {0x10d90u, 0x10e5fu},
    {0x10e7fu, 0x10e7fu},
    {0x10eaau, 0x10eacu},
    {0x10eaeu, 0x10eafu},
    {0x10eb2u, 0x10ec1u},
    {0x10ec8u, 0x10ecfu},
    {0x10ed9u, 0x10effu},
    {0x10f28u, 0x10f2fu},
    {0x10f46u, 0x10f50u},
    {0x10f5au, 0x10f6fu},
    {0x10f82u, 0x10f85u},
    {0x10f8au, 0x10fafu},
    {0x10fccu, 0x10fdfu},
    {0x10ff7u, 0x10fffu},
    {0x11001u, 0x11001u},
    {0x11038u, 0x11046u},
    {0x1104eu, 0x11051u},
    {0x11070u, 0x11070u},
    {0x11073u, 0x11074u},
    {0x11076u, 0x11081u},
    {0x110b3u, 0x110b6u},
    {0x110b9u, 0x110bau},
    {0x110bdu, 0x110bdu},
    {0x110c2u, 0x110cfu},
    {0x110e9u, 0x110efu},
    {0x110fau, 0x11102u},
    {0x11127u, 0x1112bu},
    {0x1112du, 0x11135u},
    {0x11148u, 0x1114fu},
    {0x11173u, 0x11173u},
    {0x11177u, 0x11181u},
    {0x111b6u, 0x111beu},
    {0x111c9u, 0x111ccu},
    {0x111cfu, 0x111cfu},
    {0x111e0u, 0x111e0u},
    {0x111f5u, 0x111ffu},
    {0x11212u, 0x11212u},
    {0x1122fu, 0x11231u},
    {0x11234u, 0x11234u},
    {0x11236u, 0x11237u},
    {0x1123eu, 0x1123eu},
    {0x11241u, 0x1127fu},
    {0x11287u, 0x11287u},
    {0x11289u, 0x11289u},
    {0x1128eu, 0x1128eu},
    {0x1129eu, 0x1129eu},
    {0x112aau, 0x112afu},
    {0x112dfu, 0x112dfu},
    {0x112e3u, 0x112efu},
    {0x112fau, 0x11301u},
    {0x11304u, 0x11304u},
    {0x1130du, 0x1130eu},
    {0x11311u, 0x11312u},
    {0x11329u, 0x11329u},
    {0x11331u, 0x11331u},
    {0x11334u, 0x11334u},
    {0x1133au, 0x1133cu},
    {0x11340u, 0x11340u},
    {0x11345u, 0x11346u},
    {0x11349u, 0x1134au},
    {0x1134eu, 0x1134fu},
    {0x11351u, 0x11356u},
    {0x11358u, 0x1135cu},
    {0x11364u, 0x1137fu},
    {0x1138au, 0x1138au},
    {0x1138cu, 0x1138du},
    {0x1138fu, 0x1138fu},
    {0x113b6u, 0x113b6u},
    {0x113bbu, 0x113c1u},
    {0x113c3u, 0x113c4u},
    {0x113c6u, 0x113c6u},
    {0x113cbu, 0x113cbu},
    {0x113ceu, 0x113ceu},
    {0x113d0u, 0x113d0u},
    {0x113d2u, 0x113d2u},
    {0x113d6u, 0x113d6u},
    {0x113d9u, 0x113ffu},
    {0x11438u, 0x1143fu},
    {0x11442u, 0x11444u},
    {0x11446u, 0x11446u},
    {0x1145cu, 0x1145cu},
    {0x1145eu, 0x1145eu},
    {0x11462u, 0x1147fu},
    {0x114b3u, 0x114b8u},
    {0x114bau, 0x114bau},
    {0x114bfu, 0x114c0u},
    {0x114c2u, 0x114c3u},
    {0x114c8u, 0x114cfu},
    {0x114dau, 0x1157fu},
    {0x115b2u, 0x115b7u},
    {0x115bcu, 0x115bdu},
    {0x115bfu, 0x115c0u},
    {0x115dcu, 0x115ffu},
    {0x11633u, 0x1163au},
    {0x1163du, 0x1163du},
    {0x1163fu, 0x11640u},
    {0x11645u, 0x1164fu},
    {0x1165au, 0x1165fu},
    {0x1166du, 0x1167fu},
    {0x116abu, 0x116abu},
    {0x116adu, 0x116adu},
    {0x116b0u, 0x116b5u},
    {0x116b7u, 0x116b7u},
    {0x116bau, 0x116bfu},
    {0x116cau, 0x116cfu},
    {0x116e4u, 0x116ffu},
    {0x1171bu, 0x1171du},
    {0x1171fu, 0x1171fu},
    {0x11722u, 0x11725u},
    {0x11727u, 0x1172fu},
    {0x11747u, 0x117ffu},
    {0x1182fu, 0x11837u},
    {0x11839u, 0x1183au},
    {0x1183cu, 0x1189fu},
    {0x118f3u, 0x118feu},
    {0x11907u, 0x11908u},
    {0x1190au, 0x1190bu},
    {0x11914u, 0x11914u},
    {0x11917u, 0x11917u},
    {0x11936u, 0x11936u},
    {0x11939u, 0x1193cu},
    {0x1193eu, 0x1193eu},
    {0x11943u, 0x11943u},
    {0x11947u, 0x1194fu},
    {0x1195au, 0x1199fu},
    {0x119a8u, 0x119a9u},
    {0x119d4u, 0x119dbu},
    {0x119e0u, 0x119e0u},
    {0x119e5u, 0x119ffu},
    {0x11a01u, 0x11a0au},
    {0x11a33u, 0x11a38u},
    {0x11a3bu, 0x11a3eu},
    {0x11a47u, 0x11a4fu},
    {0x11a51u, 0x11a56u},
    {0x11a59u, 0x11a5bu},
    {0x11a8au, 0x11a96u},
    {0x11a98u, 0x11a99u},
    {0x11aa3u, 0x11aafu},
    {0x11af9u, 0x11affu},
    {0x11b0au, 0x11b60u},
    {0x11b62u, 0x11b64u},
    {0x11b66u, 0x11b66u},
    {0x11b68u, 0x11bbfu},
    {0x11be2u, 0x11befu},
    {0x11bfau, 0x11bffu},
    {0x11c09u, 0x11c09u},
    {0x11c30u, 0x11c3du},
    {0x11c3fu, 0x11c3fu},
    {0x11c46u, 0x11c4fu},
    {0x11c6du, 0x11c6fu},
    {0x11c90u, 0x11ca8u},
    {0x11caau, 0x11cb0u},
    {0x11cb2u, 0x11cb3u},
    {0x11cb5u, 0x11cffu},
    {0x11d07u, 0x11d07u},
    {0x11d0au, 0x11d0au},
    {0x11d31u, 0x11d45u},
    {0x11d47u, 0x11d4fu},
    {0x11d5au, 0x11d5fu},
    {0x11d66u, 0x11d66u},
    {0x11d69u, 0x11d69u},
    {0x11d8fu, 0x11d92u},
    {0x11d95u, 0x11d95u},
    {0x11d97u, 0x11d97u},
    {0x11d99u, 0x11d9fu},
    {0x11daau, 0x11dafu},
    {0x11ddcu, 0x11ddfu},
    {0x11deau, 0x11edfu},
    {0x11ef3u, 0x11ef4u},
    {0x11ef9u, 0x11f01u},
    {0x11f11u, 0x11f11u},
    {0x11f36u, 0x11f3du},
    {0x11f40u, 0x11f40u},
    {0x11f42u, 0x11f42u},
    {0x11f5au, 0x11fafu},
    {0x11fb1u, 0x11fbfu},
    {0x11ff2u, 0x11ffeu},
    {0x1239au, 0x123ffu},
    {0x1246fu, 0x1246fu},
    {0x12475u, 0x1247fu},
    {0x12544u, 0x12f8fu},
    {0x12ff3u, 0x12fffu},
    {0x13430u, 0x13440u},
    {0x13447u, 0x1345fu},
    {0x143fbu, 0x143ffu},
    {0x14647u, 0x160ffu},
    {0x1611eu, 0x16129u},
    {0x1612du, 0x1612fu},
    {0x1613au, 0x167ffu},
    {0x16a39u, 0x16a3fu},
    {0x16a5fu, 0x16a5fu},
    {0x16a6au, 0x16a6du},
    {0x16abfu, 0x16abfu},
    {0x16acau, 0x16acfu},
    {0x16aeeu, 0x16af4u},
    {0x16af6u, 0x16affu},
    {0x16b30u, 0x16b36u},
    {0x16b46u, 0x16b4fu},
    {0x16b5au, 0x16b5au},
    {0x16b62u, 0x16b62u},
    {0x16b78u, 0x16b7cu},
    {0x16b90u, 0x16d3fu},
    {0x16d7au, 0x16e3fu},
    {0x16e9bu, 0x16e9fu},
    {0x16eb9u, 0x16ebau},
    {0x16ed4u, 0x16effu},
    {0x16f4bu, 0x16f4fu},
    {0x16f88u, 0x16f92u},
    {0x16fa0u, 0x16fdfu},
    {0x16fe4u, 0x16fefu},
    {0x16ff7u, 0x16fffu},
    {0x18cd6u, 0x18cfeu},
    {0x18d1fu, 0x18d7fu},
    {0x18df3u, 0x1afefu},
    {0x1aff4u, 0x1aff4u},
    {0x1affcu, 0x1affcu},
    {0x1afffu, 0x1afffu},
    {0x1b123u, 0x1b131u},
    {0x1b133u, 0x1b14fu},
    {0x1b153u, 0x1b154u},
    {0x1b156u, 0x1b163u},
    {0x1b168u, 0x1b16fu},
    {0x1b2fcu, 0x1bbffu},
    {0x1bc6bu, 0x1bc6fu},
    {0x1bc7du, 0x1bc7fu},
    {0x1bc89u, 0x1bc8fu},
    {0x1bc9au, 0x1bc9bu},
    {0x1bc9du, 0x1bc9eu},
    {0x1bca0u, 0x1cbffu},
    {0x1ccfdu, 0x1ccffu},
    {0x1ceb4u, 0x1ceb9u},
    {0x1ced1u, 0x1cedfu},
    {0x1cef1u, 0x1cf4fu},
    {0x1cfc4u, 0x1cfffu},
    {0x1d0f6u, 0x1d0ffu},
    {0x1d127u, 0x1d128u},
    {0x1d167u, 0x1d169u},
    {0x1d173u, 0x1d182u},
    {0x1d185u, 0x1d18bu},
    {0x1d1aau, 0x1d1adu},
    {0x1d1ebu, 0x1d1ffu},
    {0x1d242u, 0x1d244u},
    {0x1d246u, 0x1d2bfu},
    {0x1d2d4u, 0x1d2dfu},
    {0x1d2f4u, 0x1d2ffu},
    {0x1d357u, 0x1d35fu},
    {0x1d379u, 0x1d3ffu},
    {0x1d455u, 0x1d455u},
    {0x1d49du, 0x1d49du},
    {0x1d4a0u, 0x1d4a1u},
    {0x1d4a3u, 0x1d4a4u},
    {0x1d4a7u, 0x1d4a8u},
    {0x1d4adu, 0x1d4adu},
    {0x1d4bau, 0x1d4bau},
    {0x1d4bcu, 0x1d4bcu},
    {0x1d4c4u, 0x1d4c4u},
    {0x1d506u, 0x1d506u},
    {0x1d50bu, 0x1d50cu},
    {0x1d515u, 0x1d515u},
    {0x1d51du, 0x1d51du},
    {0x1d53au, 0x1d53au},
    {0x1d53fu, 0x1d53fu},
    {0x1d545u, 0x1d545u},
    {0x1d547u, 0x1d549u},
    {0x1d551u, 0x1d551u},
    {0x1d6a6u, 0x1d6a7u},
    {0x1d7ccu, 0x1d7cdu},
    {0x1da00u, 0x1da36u},
    {0x1da3bu, 0x1da6cu},
    {0x1da75u, 0x1da75u},
    {0x1da84u, 0x1da84u},
    {0x1da8cu, 0x1deffu},
    {0x1df1fu, 0x1df24u},
    {0x1df2bu, 0x1e02fu},
    {0x1e06eu, 0x1e0ffu},
    {0x1e12du, 0x1e136u},
    {0x1e13eu, 0x1e13fu},
    {0x1e14au, 0x1e14du},
    {0x1e150u, 0x1e28fu},
    {0x1e2aeu, 0x1e2bfu},
    {0x1e2ecu, 0x1e2efu},
    {0x1e2fau, 0x1e2feu},
    {0x1e300u, 0x1e4cfu},
    {0x1e4ecu, 0x1e4efu},
    {0x1e4fau, 0x1e5cfu},
    {0x1e5eeu, 0x1e5efu},
    {0x1e5fbu, 0x1e5feu},
    {0x1e600u, 0x1e6bfu},
    {0x1e6dfu, 0x1e6dfu},
    {0x1e6e3u, 0x1e6e3u},
    {0x1e6e6u, 0x1e6e6u},
    {0x1e6eeu, 0x1e6efu},
    {0x1e6f5u, 0x1e6fdu},
    {0x1e700u, 0x1e7dfu},
    {0x1e7e7u, 0x1e7e7u},
    {0x1e7ecu, 0x1e7ecu},
    {0x1e7efu, 0x1e7efu},
    {0x1e7ffu, 0x1e7ffu},
    {0x1e8c5u, 0x1e8c6u},
    {0x1e8d0u, 0x1e8ffu},
    {0x1e944u, 0x1e94au},
    {0x1e94cu, 0x1e94fu},
    {0x1e95au, 0x1e95du},
    {0x1e960u, 0x1ec70u},
    {0x1ecb5u, 0x1ed00u},
    {0x1ed3eu, 0x1edffu},
    {0x1ee04u, 0x1ee04u},
    {0x1ee20u, 0x1ee20u},
    {0x1ee23u, 0x1ee23u},
    {0x1ee25u, 0x1ee26u},
    {0x1ee28u, 0x1ee28u},
    {0x1ee33u, 0x1ee33u},
    {0x1ee38u, 0x1ee38u},
    {0x1ee3au, 0x1ee3au},
    {0x1ee3cu, 0x1ee41u},
    {0x1ee43u, 0x1ee46u},
    {0x1ee48u, 0x1ee48u},
    {0x1ee4au, 0x1ee4au},
    {0x1ee4cu, 0x1ee4cu},
    {0x1ee50u, 0x1ee50u},
    {0x1ee53u, 0x1ee53u},
    {0x1ee55u, 0x1ee56u},
    {0x1ee58u, 0x1ee58u},
    {0x1ee5au, 0x1ee5au},
    {0x1ee5cu, 0x1ee5cu},
    {0x1ee5eu, 0x1ee5eu},
    {0x1ee60u, 0x1ee60u},
    {0x1ee63u, 0x1ee63u},
    {0x1ee65u, 0x1ee66u},
    {0x1ee6bu, 0x1ee6bu},
    {0x1ee73u, 0x1ee73u},
    {0x1ee78u, 0x1ee78u},
    {0x1ee7du, 0x1ee7du},
    {0x1ee7fu, 0x1ee7fu},
    {0x1ee8au, 0x1ee8au},
    {0x1ee9cu, 0x1eea0u},
    {0x1eea4u, 0x1eea4u},
    {0x1eeaau, 0x1eeaau},
    {0x1eebcu, 0x1eeefu},
    {0x1eef2u, 0x1efffu},
    {0x1f02cu, 0x1f02fu},
    {0x1f094u, 0x1f09fu},
    {0x1f0afu, 0x1f0b0u},
    {0x1f0c0u, 0x1f0c0u},
    {0x1f0d0u, 0x1f0d0u},
    {0x1f0f6u, 0x1f0ffu},
    {0x1f1aeu, 0x1f1e5u},
    {0x1f203u, 0x1f20fu},
    {0x1f23cu, 0x1f23fu},
    {0x1f249u, 0x1f24fu},
    {0x1f252u, 0x1f25fu},
    {0x1f266u, 0x1f2ffu},
    {0x1f6d9u, 0x1f6dbu},
    {0x1f6edu, 0x1f6efu},
    {0x1f6fdu, 0x1f6ffu},
    {0x1f7dau, 0x1f7dfu},
    {0x1f7ecu, 0x1f7efu},
    {0x1f7f1u, 0x1f7ffu},
    {0x1f80cu, 0x1f80fu},
    {0x1f848u, 0x1f84fu},
    {0x1f85au, 0x1f85fu},
    {0x1f888u, 0x1f88fu},
    {0x1f8aeu, 0x1f8afu},
    {0x1f8bcu, 0x1f8bfu},
    {0x1f8c2u, 0x1f8cfu},
    {0x1f8d9u, 0x1f8ffu},
    {0x1fa58u, 0x1fa5fu},
    {0x1fa6eu, 0x1fa6fu},
    {0x1fa7du, 0x1fa7fu},
    {0x1fa8bu, 0x1fa8du},
    {0x1fac7u, 0x1fac7u},
    {0x1fac9u, 0x1faccu},
    {0x1faddu, 0x1fadeu},
    {0x1faebu, 0x1faeeu},
    {0x1faf9u, 0x1faffu},
    {0x1fb93u, 0x1fb93u},
    {0x1fbfbu, 0x1ffffu},
    {0x2a6e0u, 0x2a6ffu},
    {0x2b81eu, 0x2b81fu},
    {0x2ceaeu, 0x2ceafu},
    {0x2ebe1u, 0x2ebefu},
    {0x2ee5eu, 0x2f7ffu},
    {0x2fa1eu, 0x2ffffu},
    {0x3134bu, 0x3134fu},
    {0x3347au, 0x10ffffu},
};
static int hl_debug_is_nonprinting(unsigned int codepoint) {
    size_t low = 0;
    size_t high = sizeof(hl_nonprinting_ranges) / sizeof(hl_nonprinting_ranges[0]);
    while (low < high) {
        size_t middle = low + (high - low) / 2;
        if (codepoint < hl_nonprinting_ranges[middle].first) high = middle;
        else if (codepoint > hl_nonprinting_ranges[middle].last) low = middle + 1;
        else return 1;
    }
    return 0;
}
static void hl_debug_string(const char *value, size_t length) {
    putchar('"');
    for (size_t i = 0; value && i < length;) {
        const unsigned char *bytes = (const unsigned char *)value + i;
        size_t width = 1;
        unsigned int codepoint = hl_debug_codepoint(bytes, length - i, &width);
        switch (codepoint) {
            case 0: fputs("\\0", stdout); break;
            case '"': fputs("\\\"", stdout); break;
            case '\\': fputs("\\\\", stdout); break;
            case '\n': fputs("\\n", stdout); break;
            case '\r': fputs("\\r", stdout); break;
            case '\t': fputs("\\t", stdout); break;
            default:
                /* Match Rust Debug's non-printing Unicode categories independent of locale. */
                if (hl_debug_is_nonprinting(codepoint))
                    printf("\\u{%x}", codepoint);
                else
                    fwrite(bytes, 1, width, stdout);
        }
        i += width;
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
    if (left_length > SIZE_MAX - 3 || right_length > SIZE_MAX - left_length - 3)
        hl_fail(environment, "cadena demasiado grande");
    size_t result_length = left_length + right_length + 2;
    char *joined = (char *)hl_alloc(environment, result_length + 1);
    memcpy(joined, left_text, left_length);
    joined[left_length] = '(';
    memcpy(joined + left_length + 1, right_text, right_length);
    joined[left_length + right_length + 1] = ')';
    joined[result_length] = '\0';
    return hl_string_n(environment, joined, result_length);
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
        return hl_string_n(environment, joined, result_length);
    }
    if (left.tag == HL_NUMBER && right.tag == HL_STRING) {
        const char *number = hl_to_text(environment, left);
        size_t number_length = strlen(number);
        if (right.string_length == SIZE_MAX
            || number_length > SIZE_MAX - right.string_length - 1)
            hl_fail(environment, "cadena demasiado grande");
        size_t result_length = number_length + right.string_length;
        char *joined = (char *)hl_alloc(environment, result_length + 1);
        memcpy(joined, number, number_length);
        memcpy(joined + number_length, right.as.string, right.string_length);
        joined[result_length] = '\0';
        return hl_string_n(environment, joined, result_length);
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
