/* What a program built with -finstrument-functions called, and from where:
 * each pair of a function entered and the address its call returns to,
 * once, written at exit to GRAFF_CYG_TRACE/<pid> as two hexadecimal
 * addresses a line, after a first line naming the program's file. Linked
 * into a program built -no-pie, so that the addresses are the program
 * file's own, which addr2line reads. Pairs are kept in a table of fixed
 * size filled by compare-and-swap, as OpenMP's threads call at once; a
 * pair past its room is counted, and the count written last. */

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>

#define SLOTS (1u << 20)

static uint64_t pairs[SLOTS];
static unsigned long lost;

#define QUIET __attribute__((no_instrument_function))

QUIET static uint64_t mix(uint64_t key) {
    key ^= key >> 33;
    key *= 0xff51afd7ed558ccdULL;
    key ^= key >> 33;
    return key;
}

QUIET void __cyg_profile_func_enter(void *function, void *site) {
    /* A program built -no-pie lies below 4 GiB: both halves fit a word. A
     * call from a library, as OpenMP's runtime calls a parallel region, has
     * its site above: written as 0, no site. */
    uintptr_t from = (uintptr_t)site <= 0xffffffffu ? (uintptr_t)site : 0;
    uint64_t key = ((uint64_t)(uintptr_t)function << 32) | (uint32_t)from;
    if (key == 0)
        return;
    uint64_t at = mix(key) & (SLOTS - 1);
    for (unsigned tries = 0; tries < 64; tries++, at = (at + 1) & (SLOTS - 1)) {
        uint64_t held = __atomic_load_n(&pairs[at], __ATOMIC_RELAXED);
        if (held == key)
            return;
        if (held == 0) {
            uint64_t empty = 0;
            if (__atomic_compare_exchange_n(&pairs[at], &empty, key, 0, __ATOMIC_RELAXED, __ATOMIC_RELAXED))
                return;
            if (empty == key)
                return;
        }
    }
    __atomic_fetch_add(&lost, 1, __ATOMIC_RELAXED);
}

QUIET void __cyg_profile_func_exit(void *function, void *site) {
    (void)function;
    (void)site;
}

QUIET __attribute__((destructor)) static void written(void) {
    const char *folder = getenv("GRAFF_CYG_TRACE");
    if (folder == NULL)
        return;
    char path[4096], program[4096];
    snprintf(path, sizeof path, "%s/%ld", folder, (long)getpid());
    ssize_t length = readlink("/proc/self/exe", program, sizeof program - 1);
    program[length < 0 ? 0 : length] = '\0';
    FILE *out = fopen(path, "w");
    if (out == NULL)
        return;
    fprintf(out, "%s\n", program);
    for (uint64_t at = 0; at < SLOTS; at++)
        if (pairs[at] != 0)
            fprintf(out, "%llx %llx\n", (unsigned long long)(pairs[at] >> 32),
                    (unsigned long long)(pairs[at] & 0xffffffffu));
    fprintf(out, "lost %lu\n", lost);
    fclose(out);
}
