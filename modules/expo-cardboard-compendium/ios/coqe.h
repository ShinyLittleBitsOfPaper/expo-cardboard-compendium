// C ABI of the Rust engine (crates/engine/src/ffi.rs)
char *coqe_call(const char *method, const char *args);
void coqe_free(char *p);
