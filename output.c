
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// El tipo _popen y _pclose son específicos de Windows/MSVC.
// Para compatibilidad con MinGW (nuestro toolchain gnu), usamos popen/pclose.
#ifdef _WIN32
#define popen _popen
#define pclose _pclose
#endif

typedef enum { TYPE_NUMBER, TYPE_STRING, TYPE_ARRAY, TYPE_NULL } HackType;
typedef struct HackValue HackValue;
struct HackValue {
    HackType type;
    union {
        double number;
        char* string;
        struct { HackValue* elements; int length; } array;
    } as;
};

void print_value(HackValue val) {
    if (val.type == TYPE_NUMBER) { printf("%f\n", val.as.number); }
    else if (val.type == TYPE_STRING) { printf("%s\n", val.as.string); }
    else if (val.type == TYPE_ARRAY) { printf("[array of length %d]\n", val.as.array.length); }
    else { printf("(null)\n"); }
}

HackValue hack_read_file(HackValue path_val) {
    if (path_val.type != TYPE_STRING) return (HackValue){.type = TYPE_NULL};
    FILE *f = fopen(path_val.as.string, "rb");
    if (f == NULL) return (HackValue){.type = TYPE_NULL};
    fseek(f, 0, SEEK_END); long fsize = ftell(f); fseek(f, 0, SEEK_SET);
    char *content = malloc(fsize + 1); fread(content, 1, fsize, f); fclose(f);
    content[fsize] = 0;
    return (HackValue){.type = TYPE_STRING, .as.string = content};
}

HackValue hack_write_file(HackValue path_val, HackValue content_val) {
    if (path_val.type != TYPE_STRING || content_val.type != TYPE_STRING) {
        return (HackValue){.type = TYPE_NUMBER, .as.number = -1.0};
    }
    FILE *f = fopen(path_val.as.string, "w");
    if (f == NULL) return (HackValue){.type = TYPE_NUMBER, .as.number = -1.0};
    fprintf(f, "%s", content_val.as.string); fclose(f);
    return (HackValue){.type = TYPE_NUMBER, .as.number = 0.0};
}

// --- ¡NUESTRA FUNCIÓN NATIVA `exec` EN C! ---
HackValue hack_exec(HackValue command_val) {
    if (command_val.type != TYPE_STRING) return (HackValue){.type = TYPE_NULL};
    
    FILE *pipe = popen(command_val.as.string, "r");
    if (!pipe) return (HackValue){.type = TYPE_NULL};
    
    char buffer[128];
    // Empezamos con un string dinámico vacío.
    char *result = malloc(1);
    result[0] = '\0';

    // Leemos la salida del comando línea por línea.
    while (fgets(buffer, sizeof(buffer), pipe) != NULL) {
        // Agrandamos nuestro string de resultado y añadimos el nuevo trozo.
        result = realloc(result, strlen(result) + strlen(buffer) + 1);
        strcat(result, buffer);
    }
    
    pclose(pipe);
    return (HackValue){.type = TYPE_STRING, .as.string = result};
}


int main() {
    print_value((HackValue){.type = TYPE_STRING, .as.string = "Ejecutando el comando 'dir'..."});
    HackValue directory_listing = hack_exec((HackValue){.type = TYPE_STRING, .as.string = "dir"});
    print_value((HackValue){.type = TYPE_STRING, .as.string = "--- Salida del comando 'dir' ---"});
    print_value(directory_listing);
    print_value((HackValue){.type = TYPE_STRING, .as.string = "---------------------------------"});
    HackValue ping_output = hack_exec((HackValue){.type = TYPE_STRING, .as.string = "ping -n 1 google.com"});
    print_value((HackValue){.type = TYPE_STRING, .as.string = "--- Salida del comando 'ping' ---"});
    print_value(ping_output);
    print_value((HackValue){.type = TYPE_STRING, .as.string = "---------------------------------"});

    return 0;
}
