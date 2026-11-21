
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef enum { TYPE_NUMBER, TYPE_STRING, TYPE_ARRAY, TYPE_MAP, TYPE_NULL } HackType;

// Declaramos los nombres de los structs por adelantado.
struct HackValue;
struct HackPair;

// Ahora definimos la estructura completa de HackValue.
// Puede usar un puntero a HackPair porque el compilador no necesita saber el tamaño de HackPair todavía.
struct HackValue {
    HackType type;
    union {
        double number;
        char* string;
        struct { struct HackValue* elements; int length; } array;
        struct { struct HackPair* pairs; int length; } map;
    } as;
};

// Ahora que HackValue está completamente definido, podemos definir HackPair,
// que SÍ necesita saber el tamaño de HackValue.
struct HackPair {
    struct HackValue key;
    struct HackValue value;
};

// Para conveniencia, creamos un alias de tipo.
typedef struct HackValue HackValue;
typedef struct HackPair HackPair;

void print_value(HackValue val); // Declaración adelantada

HackValue map_get(HackValue map, HackValue key) {
    if (map.type != TYPE_MAP) return (HackValue){.type = TYPE_NULL};
    for (int i = 0; i < map.as.map.length; i++) {
        HackPair pair = map.as.map.pairs[i];
        if (pair.key.type == key.type) {
            if (key.type == TYPE_NUMBER && pair.key.as.number == key.as.number) { return pair.value; }
            if (key.type == TYPE_STRING && strcmp(pair.key.as.string, key.as.string) == 0) { return pair.value; }
        }
    }
    return (HackValue){.type = TYPE_NULL};
}

void print_value(HackValue val) {
    if (val.type == TYPE_NUMBER) { printf("%f\n", val.as.number); }
    else if (val.type == TYPE_STRING) { printf("%s\n", val.as.string); }
    else if (val.type == TYPE_ARRAY) { printf("[array of length %d]\n", val.as.array.length); }
    else if (val.type == TYPE_MAP) { printf("[map of size %d]\n", val.as.map.length); }
    else { printf("(null)\n"); }
}

int main() {
    HackValue target_info = ({
    HackValue* _tmp_0_elements = malloc(sizeof(HackValue) * 0);
    (HackValue){.type = TYPE_ARRAY, .as.array = {.elements = _tmp_0_elements, .length = 0}};
});
    print_value((HackValue){.type = TYPE_STRING, .as.string = "Escaneando host:"});
    print_value(map_get(target_info, (HackValue){.type = TYPE_STRING, .as.string = "host"}));
    print_value((HackValue){.type = TYPE_STRING, .as.string = "En el puerto:"});
    print_value(map_get(target_info, (HackValue){.type = TYPE_STRING, .as.string = "port"}));

    return 0;
}
