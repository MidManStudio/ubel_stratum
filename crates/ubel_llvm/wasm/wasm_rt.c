/* Freestanding runtime for emitted programs on wasm32: no libc, no OS.
 * Printing and traps are imports that the JavaScript host provides.
 * Arenas are a counter plus a small pool of headers: the Stage 1B subset
 * creates and destroys arenas but does not allocate from them yet. */
typedef long long i64;
extern void js_print_i64(i64 v);
extern void js_print_bool(int b);
extern void js_print_str(const char *s, i64 len);
extern void js_trap(int code);

static unsigned long live;
static char pool[4096][16];
static unsigned long next_slot;

void *ubel_arena_create(i64 chunk_size) {
    (void)chunk_size;
    live++;
    return pool[next_slot++ & 4095];
}
void ubel_arena_destroy(void *a) { (void)a; live--; }

void ubel_print_i64(i64 v)                 { js_print_i64(v); }
void ubel_print_bool(int b)                { js_print_bool(b); }
void ubel_print_str(const char *s, i64 n)  { js_print_str(s, n); }
void ubel_print_nl(void)                   { js_print_str("\n", 1); }
void ubel_trap_div_zero(void)              { js_trap(1); }
void ubel_trap_no_return(void)             { js_trap(2); }
unsigned long ubel_arena_live_count(void)  { return live; }
