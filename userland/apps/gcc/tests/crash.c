/* Deliberately dies by SIGSEGV so the booted suite can observe the
 * wait4 WIFSIGNALED encoding end-to-end through a freshly compiled
 * binary. The volatile store defeats constant-folding of the fault. */
int main(void) {
    volatile int *p = 0;
    *p = 42;
    return 0;
}
