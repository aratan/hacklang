/*
 * HackLang native runtime — first supported subset.
 * Values are intentionally dynamic; the Rust checker is not a lowering requirement.
 */
#include <float.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef enum { HL_NUMBER, HL_BOOL, HL_STRING } HlTag;
typedef struct {
    HlTag tag;
    union { double number; int boolean; const char *string; } as;
} HlValue;

typedef struct HlBinding {
    const char *name;
    HlValue value;
    struct HlBinding *next;
} HlBinding;

typedef struct HlEnv {
    struct HlEnv *parent;
    HlBinding *bindings;
} HlEnv;

static void hl_fail(const char *message) {
    fprintf(stderr, "Error en tiempo de ejecución: %s\n", message);
    exit(1);
}

static void *hl_alloc(size_t size) {
    void *memory = malloc(size);
    if (!memory) hl_fail("memoria agotada");
    return memory;
}

static char *hl_copy_string(const char *value) {
    size_t size = strlen(value) + 1;
    char *copy = (char *)hl_alloc(size);
    memcpy(copy, value, size);
    return copy;
}

static HlValue hl_number(double value) {
    HlValue result; result.tag = HL_NUMBER; result.as.number = value; return result;
}
static HlValue hl_bool(int value) {
    HlValue result; result.tag = HL_BOOL; result.as.boolean = value != 0; return result;
}
static HlValue hl_string(const char *value) {
    HlValue result; result.tag = HL_STRING; result.as.string = hl_copy_string(value); return result;
}
static HlEnv *hl_env_new(HlEnv *parent) {
    HlEnv *environment = (HlEnv *)hl_alloc(sizeof(HlEnv));
    environment->parent = parent;
    environment->bindings = NULL;
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
static void hl_define(HlEnv *environment, const char *name, HlValue value) {
    HlBinding *item = hl_find_local(environment, name);
    if (item) { item->value = value; return; }
    item = (HlBinding *)hl_alloc(sizeof(HlBinding));
    item->name = hl_copy_string(name);
    item->value = value;
    item->next = environment->bindings;
    environment->bindings = item;
}
static void hl_assign(HlEnv *environment, const char *name, HlValue value) {
    HlBinding *item = hl_find(environment, name);
    if (!item) {
        char message[512];
        snprintf(message, sizeof(message), "Variable '%s' no definida para asignación", name);
        hl_fail(message);
    }
    item->value = value;
}
static HlValue hl_get(HlEnv *environment, const char *name) {
    HlBinding *item = hl_find(environment, name);
    if (!item) {
        char message[512];
        snprintf(message, sizeof(message), "Variable '%s' no definida", name);
        hl_fail(message);
    }
    return item->value;
}
static int hl_truthy(HlValue value) {
    switch (value.tag) {
        case HL_BOOL: return value.as.boolean;
        case HL_NUMBER: return value.as.number != 0.0;
        case HL_STRING: return value.as.string[0] != '\0';
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
static const char *hl_to_text(HlValue value) {
    if (value.tag == HL_STRING) return value.as.string;
    if (value.tag == HL_BOOL) return value.as.boolean ? "true" : "false";
    char buffer[512];
    hl_number_text(value.as.number, buffer, sizeof(buffer));
    return hl_copy_string(buffer);
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
        case HL_STRING: puts(value.as.string); break;
    }
}
static int hl_is_string_number_pair(HlValue left, HlValue right) {
    return (left.tag == HL_STRING && right.tag == HL_NUMBER)
        || (left.tag == HL_NUMBER && right.tag == HL_STRING);
}
static HlValue hl_mixed_text(HlValue left, HlValue right) {
    const char *left_text = hl_to_text(left);
    const char *right_text = hl_to_text(right);
    size_t size = strlen(left_text) + strlen(right_text) + 3;
    char *joined = (char *)hl_alloc(size);
    snprintf(joined, size, "%s(%s)", left_text, right_text);
    return hl_string(joined);
}
static void hl_require_numbers(HlValue left, HlValue right, const char *operation) {
    if (left.tag != HL_NUMBER || right.tag != HL_NUMBER) {
        char message[256];
        snprintf(message, sizeof(message), "Type mismatch: valores incompatibles con el operador %s", operation);
        hl_fail(message);
    }
}
static HlValue hl_add(HlValue left, HlValue right) {
    if (left.tag == HL_NUMBER && right.tag == HL_NUMBER)
        return hl_number(left.as.number + right.as.number);
    if (left.tag == HL_STRING && right.tag == HL_NUMBER) {
        const char *number = hl_to_text(right);
        size_t size = strlen(left.as.string) + strlen(number) + 1;
        char *joined = (char *)hl_alloc(size);
        snprintf(joined, size, "%s%s", left.as.string, number);
        return hl_string(joined);
    }
    if (left.tag == HL_NUMBER && right.tag == HL_STRING) {
        const char *number = hl_to_text(left);
        size_t size = strlen(number) + strlen(right.as.string) + 1;
        char *joined = (char *)hl_alloc(size);
        snprintf(joined, size, "%s%s", number, right.as.string);
        return hl_string(joined);
    }
    hl_fail("Type mismatch: valores incompatibles con el operador Plus");
    return hl_number(0);
}
static HlValue hl_sub(HlValue left, HlValue right) {
    if (hl_is_string_number_pair(left, right)) return hl_mixed_text(left, right);
    hl_require_numbers(left, right, "Minus"); return hl_number(left.as.number - right.as.number);
}
static HlValue hl_mul(HlValue left, HlValue right) {
    if (hl_is_string_number_pair(left, right)) return hl_mixed_text(left, right);
    hl_require_numbers(left, right, "Star"); return hl_number(left.as.number * right.as.number);
}
static HlValue hl_div(HlValue left, HlValue right) {
    if (hl_is_string_number_pair(left, right)) return hl_mixed_text(left, right);
    hl_require_numbers(left, right, "Slash");
    if (right.as.number == 0.0) hl_fail("Division by zero");
    return hl_number(left.as.number / right.as.number);
}
static HlValue hl_less(HlValue left, HlValue right) {
    if (hl_is_string_number_pair(left, right)) return hl_mixed_text(left, right);
    hl_require_numbers(left, right, "Less"); return hl_bool(left.as.number < right.as.number);
}
static HlValue hl_greater(HlValue left, HlValue right) {
    if (hl_is_string_number_pair(left, right)) return hl_mixed_text(left, right);
    hl_require_numbers(left, right, "Greater"); return hl_bool(left.as.number > right.as.number);
}
static HlValue hl_equal(HlValue left, HlValue right) {
    if (hl_is_string_number_pair(left, right)) return hl_mixed_text(left, right);
    hl_require_numbers(left, right, "Tilde"); return hl_bool(left.as.number == right.as.number);
}
static HlValue hl_not_equal(HlValue left, HlValue right) {
    if (hl_is_string_number_pair(left, right)) return hl_mixed_text(left, right);
    hl_require_numbers(left, right, "Caret");
    return hl_bool(fabs(left.as.number - right.as.number) > DBL_EPSILON);
}
