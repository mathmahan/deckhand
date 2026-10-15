/* The sizes and a few offsets of sigil's structs as its own header lays them out, for the bindings'
 * tests to hold `ffi.rs` to — see build.rs. */
#include <stddef.h>
#include <sigil.h>

size_t deckhand_sigil_sizeof_result(void) { return sizeof(sigil_result); }
size_t deckhand_sigil_sizeof_support(void) { return sizeof(sigil_support); }
size_t deckhand_sigil_sizeof_options(void) { return sizeof(sigil_options); }
size_t deckhand_sigil_sizeof_save_member(void) { return sizeof(sigil_save_member); }
size_t deckhand_sigil_sizeof_save_request(void) { return sizeof(sigil_save_request); }
size_t deckhand_sigil_sizeof_save_unit(void) { return sizeof(sigil_save_unit); }
size_t deckhand_sigil_sizeof_layout_info(void) { return sizeof(sigil_layout_info); }
size_t deckhand_sigil_sizeof_sync_request(void) { return sizeof(sigil_sync_request); }
size_t deckhand_sigil_sizeof_sync_result(void) { return sizeof(sigil_sync_result); }
size_t deckhand_sigil_offsetof_sync_result_unowned_changed(void) { return offsetof(sigil_sync_result, unowned_changed); }
size_t deckhand_sigil_offsetof_sync_request_repair(void) { return offsetof(sigil_sync_request, repair); }
size_t deckhand_sigil_offsetof_save_unit_alternate_count(void) { return offsetof(sigil_save_unit, alternate_count); }
size_t deckhand_sigil_offsetof_result_n64_md5_n64(void) { return offsetof(sigil_result, n64_md5_n64); }
