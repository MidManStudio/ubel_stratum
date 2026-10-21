; crates/llvm_poc/bench/boundaries.ll
;
; Stage 1A: how the tiers talk to each other, in hand-written LLVM IR.
; Modelled on examples/arena_demo/ast_builder.ubl: a MID function builds an
; AST in an arena and hands a pointer into it to a HIGH closure, which returns
; a GC-owned result.
;
; Cross-tier call matrix (web/site/src/tier-model.md):
;   HIGH -> MID   plain call            (xcb_*, xit_*)
;   HIGH -> LOW   plain call            (xlow_from_high)
;   MID  -> LOW   plain call            (xlow_from_mid)
;   MID  -> HIGH  only through a function-typed parameter (the callback)
;   LOW  -> anything  never; low_sum below calls nothing and allocates nothing
;
; Closure ABI proposed here: a closure is a GC object whose FIRST WORD is the
; code pointer and whose remaining words are the captured variables. The code
; receives the closure object itself as a hidden first argument:
;     ptr code(ptr closure, ptr argument)
; so a callback is one indirect call, and captured state is reachable from the
; closure with no extra allocation.
;
; Every exported benchmark returns the same checksum for the same inputs, in
; every variant, so the harness can check them against each other:
;     sum over reps of ((value_of_tree(rep) + 3) ^ (rep + 1)), plus reps
;
; Like tiers.ll this file has no `target datalayout` or `target triple`, and
; sizes and pointer masks are derived from the target's data layout.

; run_bench.sh rewrites the next line to i32 when the target's size_t is 32-bit.
%size_t = type i64

%Arena = type { ptr, ptr, ptr, i64 }       ; cur, end, chunks, chunk_size (first four fields of ubel_arena)
%Expr  = type { i64, i64, ptr, ptr }       ; kind (0 int, 1 add, 2 mul), value, lhs, rhs
%Box   = type { i64, i64 }                 ; GC-owned result: value, tag (no pointers)
%Clo   = type { ptr, i64, ptr }            ; closure: code pointer, captured k, captured counter (a GC Box)
%Item  = type { i64, i64 }                 ; arena-resident work item
%Res   = type { ptr, ptr }                 ; GC list node: box, next

declare ptr  @ubel_gc_alloc(i64, i64)
declare ptr  @ubel_arena_create(i64)
declare ptr  @ubel_arena_create_scan(i64)
declare ptr  @ubel_arena_grow(ptr, i64)
declare void @ubel_arena_destroy(ptr)
declare ptr  @malloc(%size_t)
declare void @free(ptr)

!0 = !{!"branch_weights", i32 2000, i32 1}

; ============================================================== layout helpers

; Pointer-width-independent sizes, malloc sizes, arena sizes (rounded up to 16) and
; GC pointer masks, all derived from the target's data layout.
define internal i64 @Expr_size() alwaysinline nounwind readnone {
entry:
  ret i64 ptrtoint (ptr getelementptr (%Expr, ptr null, i32 1) to i64)
}
define internal %size_t @Expr_msize() alwaysinline nounwind readnone {
entry:
  ret %size_t ptrtoint (ptr getelementptr (%Expr, ptr null, i32 1) to %size_t)
}
define internal i64 @Expr_asize() alwaysinline nounwind readnone {
entry:
  %s = call i64 @Expr_size()
  %a = add i64 %s, 15
  %r = and i64 %a, -16
  ret i64 %r
}
define internal i64 @Expr_ptrmask() alwaysinline nounwind readnone {
entry:
  %psz = ptrtoint ptr getelementptr (ptr, ptr null, i32 1) to i64
  %off0 = ptrtoint ptr getelementptr (%Expr, ptr null, i32 0, i32 2) to i64
  %idx0 = udiv i64 %off0, %psz
  %m0 = shl i64 1, %idx0
  %off1 = ptrtoint ptr getelementptr (%Expr, ptr null, i32 0, i32 3) to i64
  %idx1 = udiv i64 %off1, %psz
  %m1 = shl i64 1, %idx1
  %or1 = or i64 %m0, %m1
  ret i64 %or1
}

define internal i64 @Box_size() alwaysinline nounwind readnone {
entry:
  ret i64 ptrtoint (ptr getelementptr (%Box, ptr null, i32 1) to i64)
}
define internal %size_t @Box_msize() alwaysinline nounwind readnone {
entry:
  ret %size_t ptrtoint (ptr getelementptr (%Box, ptr null, i32 1) to %size_t)
}
define internal i64 @Box_asize() alwaysinline nounwind readnone {
entry:
  %s = call i64 @Box_size()
  %a = add i64 %s, 15
  %r = and i64 %a, -16
  ret i64 %r
}
define internal i64 @Box_ptrmask() alwaysinline nounwind readnone {
entry:
  ret i64 0
}

define internal i64 @Clo_size() alwaysinline nounwind readnone {
entry:
  ret i64 ptrtoint (ptr getelementptr (%Clo, ptr null, i32 1) to i64)
}
define internal %size_t @Clo_msize() alwaysinline nounwind readnone {
entry:
  ret %size_t ptrtoint (ptr getelementptr (%Clo, ptr null, i32 1) to %size_t)
}
define internal i64 @Clo_asize() alwaysinline nounwind readnone {
entry:
  %s = call i64 @Clo_size()
  %a = add i64 %s, 15
  %r = and i64 %a, -16
  ret i64 %r
}
define internal i64 @Clo_ptrmask() alwaysinline nounwind readnone {
entry:
  %psz = ptrtoint ptr getelementptr (ptr, ptr null, i32 1) to i64
  %off0 = ptrtoint ptr getelementptr (%Clo, ptr null, i32 0, i32 2) to i64
  %idx0 = udiv i64 %off0, %psz
  %m0 = shl i64 1, %idx0
  ret i64 %m0
}

define internal i64 @Item_size() alwaysinline nounwind readnone {
entry:
  ret i64 ptrtoint (ptr getelementptr (%Item, ptr null, i32 1) to i64)
}
define internal %size_t @Item_msize() alwaysinline nounwind readnone {
entry:
  ret %size_t ptrtoint (ptr getelementptr (%Item, ptr null, i32 1) to %size_t)
}
define internal i64 @Item_asize() alwaysinline nounwind readnone {
entry:
  %s = call i64 @Item_size()
  %a = add i64 %s, 15
  %r = and i64 %a, -16
  ret i64 %r
}
define internal i64 @Item_ptrmask() alwaysinline nounwind readnone {
entry:
  ret i64 0
}

define internal i64 @Res_size() alwaysinline nounwind readnone {
entry:
  ret i64 ptrtoint (ptr getelementptr (%Res, ptr null, i32 1) to i64)
}
define internal %size_t @Res_msize() alwaysinline nounwind readnone {
entry:
  ret %size_t ptrtoint (ptr getelementptr (%Res, ptr null, i32 1) to %size_t)
}
define internal i64 @Res_asize() alwaysinline nounwind readnone {
entry:
  %s = call i64 @Res_size()
  %a = add i64 %s, 15
  %r = and i64 %a, -16
  ret i64 %r
}
define internal i64 @Res_ptrmask() alwaysinline nounwind readnone {
entry:
  %psz = ptrtoint ptr getelementptr (ptr, ptr null, i32 1) to i64
  %off0 = ptrtoint ptr getelementptr (%Res, ptr null, i32 0, i32 0) to i64
  %idx0 = udiv i64 %off0, %psz
  %m0 = shl i64 1, %idx0
  %off1 = ptrtoint ptr getelementptr (%Res, ptr null, i32 0, i32 1) to i64
  %idx1 = udiv i64 %off1, %psz
  %m1 = shl i64 1, %idx1
  %or1 = or i64 %m0, %m1
  ret i64 %or1
}



; ================================================================ shared pieces

define internal ptr @arena_bump(ptr %a, i64 %size) alwaysinline nounwind {
entry:
  %curp = getelementptr inbounds %Arena, ptr %a, i32 0, i32 0
  %endp = getelementptr inbounds %Arena, ptr %a, i32 0, i32 1
  %cur  = load ptr, ptr %curp
  %end  = load ptr, ptr %endp
  %new  = getelementptr i8, ptr %cur, i64 %size
  %ok   = icmp ule ptr %new, %end
  br i1 %ok, label %fast, label %slow, !prof !0
fast:
  store ptr %new, ptr %curp
  ret ptr %cur
slow:
  %p = call ptr @ubel_arena_grow(ptr %a, i64 %size)
  ret ptr %p
}

define internal void @expr_init(ptr %n, i64 %kind, i64 %value, ptr %l, ptr %r) alwaysinline nounwind {
entry:
  %p0 = getelementptr inbounds %Expr, ptr %n, i32 0, i32 0
  store i64 %kind, ptr %p0
  %p1 = getelementptr inbounds %Expr, ptr %n, i32 0, i32 1
  store i64 %value, ptr %p1
  %p2 = getelementptr inbounds %Expr, ptr %n, i32 0, i32 2
  store ptr %l, ptr %p2
  %p3 = getelementptr inbounds %Expr, ptr %n, i32 0, i32 3
  store ptr %r, ptr %p3
  ret void
}

; Tier-independent: reads a tree wherever it lives (GC heap, arena or malloc).
define internal i64 @eval_expr(ptr %e) nounwind {
entry:
  %pk   = getelementptr inbounds %Expr, ptr %e, i32 0, i32 0
  %kind = load i64, ptr %pk
  %leaf = icmp eq i64 %kind, 0
  br i1 %leaf, label %leaf.bb, label %inner
leaf.bb:
  %pv = getelementptr inbounds %Expr, ptr %e, i32 0, i32 1
  %v  = load i64, ptr %pv
  ret i64 %v
inner:
  %pl = getelementptr inbounds %Expr, ptr %e, i32 0, i32 2
  %l  = load ptr, ptr %pl
  %pr = getelementptr inbounds %Expr, ptr %e, i32 0, i32 3
  %r  = load ptr, ptr %pr
  %a  = call i64 @eval_expr(ptr %l)
  %b  = call i64 @eval_expr(ptr %r)
  %isadd = icmp eq i64 %kind, 1
  %s  = add i64 %a, %b
  %m  = mul i64 %a, %b
  %res = select i1 %isadd, i64 %s, i64 %m
  ret i64 %res
}

; ---- tree builders, one per tier. Same shape and values in every tier.

define internal ptr @build_expr_gc(i64 %depth, i64 %seed) nounwind {
entry:
  %sz   = call i64 @Expr_size()
  %pm   = call i64 @Expr_ptrmask()
  %node = call ptr @ubel_gc_alloc(i64 %sz, i64 %pm)
  %leaf = icmp eq i64 %depth, 0
  br i1 %leaf, label %leaf.bb, label %rec
leaf.bb:
  %lv0 = and i64 %seed, 255
  %lv  = add i64 %lv0, 1
  call void @expr_init(ptr %node, i64 0, i64 %lv, ptr null, ptr null)
  ret ptr %node
rec:
  %d1 = sub i64 %depth, 1
  %s2 = shl i64 %seed, 1
  %l  = call ptr @build_expr_gc(i64 %d1, i64 %s2)
  %s3 = or i64 %s2, 1
  %r  = call ptr @build_expr_gc(i64 %d1, i64 %s3)
  %par  = and i64 %depth, 1
  %kind = add i64 %par, 1
  call void @expr_init(ptr %node, i64 %kind, i64 0, ptr %l, ptr %r)
  ret ptr %node
}

define internal ptr @build_expr_arena(ptr %arena, i64 %depth, i64 %seed) nounwind {
entry:
  %sz   = call i64 @Expr_asize()
  %node = call ptr @arena_bump(ptr %arena, i64 %sz)
  %leaf = icmp eq i64 %depth, 0
  br i1 %leaf, label %leaf.bb, label %rec
leaf.bb:
  %lv0 = and i64 %seed, 255
  %lv  = add i64 %lv0, 1
  call void @expr_init(ptr %node, i64 0, i64 %lv, ptr null, ptr null)
  ret ptr %node
rec:
  %d1 = sub i64 %depth, 1
  %s2 = shl i64 %seed, 1
  %l  = call ptr @build_expr_arena(ptr %arena, i64 %d1, i64 %s2)
  %s3 = or i64 %s2, 1
  %r  = call ptr @build_expr_arena(ptr %arena, i64 %d1, i64 %s3)
  %par  = and i64 %depth, 1
  %kind = add i64 %par, 1
  call void @expr_init(ptr %node, i64 %kind, i64 0, ptr %l, ptr %r)
  ret ptr %node
}

define internal ptr @build_expr_low(i64 %depth, i64 %seed) nounwind {
entry:
  %sz   = call %size_t @Expr_msize()
  %node = call ptr @malloc(%size_t %sz)
  %leaf = icmp eq i64 %depth, 0
  br i1 %leaf, label %leaf.bb, label %rec
leaf.bb:
  %lv0 = and i64 %seed, 255
  %lv  = add i64 %lv0, 1
  call void @expr_init(ptr %node, i64 0, i64 %lv, ptr null, ptr null)
  ret ptr %node
rec:
  %d1 = sub i64 %depth, 1
  %s2 = shl i64 %seed, 1
  %l  = call ptr @build_expr_low(i64 %d1, i64 %s2)
  %s3 = or i64 %s2, 1
  %r  = call ptr @build_expr_low(i64 %d1, i64 %s3)
  %par  = and i64 %depth, 1
  %kind = add i64 %par, 1
  call void @expr_init(ptr %node, i64 %kind, i64 0, ptr %l, ptr %r)
  ret ptr %node
}

define internal void @free_expr_low(ptr %e) nounwind {
entry:
  %isnull = icmp eq ptr %e, null
  br i1 %isnull, label %done, label %rec
done:
  ret void
rec:
  %pl = getelementptr inbounds %Expr, ptr %e, i32 0, i32 2
  %l  = load ptr, ptr %pl
  %pr = getelementptr inbounds %Expr, ptr %e, i32 0, i32 3
  %r  = load ptr, ptr %pr
  call void @free_expr_low(ptr %l)
  call void @free_expr_low(ptr %r)
  call void @free(ptr %e)
  ret void
}

; Deep copy of any tree into the GC heap: what `to_owned()` lowers to when a
; value built in an arena has to outlive it.
define internal ptr @copy_expr_gc(ptr %src) nounwind {
entry:
  %sz  = call i64 @Expr_size()
  %pm  = call i64 @Expr_ptrmask()
  %dst = call ptr @ubel_gc_alloc(i64 %sz, i64 %pm)
  %pk  = getelementptr inbounds %Expr, ptr %src, i32 0, i32 0
  %kind = load i64, ptr %pk
  %pv  = getelementptr inbounds %Expr, ptr %src, i32 0, i32 1
  %val = load i64, ptr %pv
  %leaf = icmp eq i64 %kind, 0
  br i1 %leaf, label %leaf.bb, label %rec
leaf.bb:
  call void @expr_init(ptr %dst, i64 0, i64 %val, ptr null, ptr null)
  ret ptr %dst
rec:
  %pl = getelementptr inbounds %Expr, ptr %src, i32 0, i32 2
  %l  = load ptr, ptr %pl
  %pr = getelementptr inbounds %Expr, ptr %src, i32 0, i32 3
  %r  = load ptr, ptr %pr
  %l2 = call ptr @copy_expr_gc(ptr %l)
  %r2 = call ptr @copy_expr_gc(ptr %r)
  call void @expr_init(ptr %dst, i64 %kind, i64 %val, ptr %l2, ptr %r2)
  ret ptr %dst
}

; ====================================================== HIGH: closures and callbacks

; Builds a HIGH closure object: [code pointer | k | counter].
define internal ptr @make_closure(ptr %code, i64 %k) nounwind {
entry:
  %bsz = call i64 @Box_size()
  %counter = call ptr @ubel_gc_alloc(i64 %bsz, i64 0)
  %csz = call i64 @Clo_size()
  %cpm = call i64 @Clo_ptrmask()
  %clo = call ptr @ubel_gc_alloc(i64 %csz, i64 %cpm)
  %p0 = getelementptr inbounds %Clo, ptr %clo, i32 0, i32 0
  store ptr %code, ptr %p0
  %p1 = getelementptr inbounds %Clo, ptr %clo, i32 0, i32 1
  store i64 %k, ptr %p1
  %p2 = getelementptr inbounds %Clo, ptr %clo, i32 0, i32 2
  store ptr %counter, ptr %p2
  ret ptr %clo
}

; HIGH callback: evaluates the arena tree it was handed a borrow of, and
; returns a GC-owned Box. It reads arena memory and writes GC memory.
define internal ptr @cb_eval(ptr %clo, ptr %node) nounwind {
entry:
  %kp = getelementptr inbounds %Clo, ptr %clo, i32 0, i32 1
  %k  = load i64, ptr %kp
  %cp = getelementptr inbounds %Clo, ptr %clo, i32 0, i32 2
  %counter = load ptr, ptr %cp
  %v  = call i64 @eval_expr(ptr %node)
  %r  = add i64 %v, %k
  %np = getelementptr inbounds %Box, ptr %counter, i32 0, i32 0
  %n  = load i64, ptr %np
  %n1 = add i64 %n, 1
  store i64 %n1, ptr %np
  %bsz = call i64 @Box_size()
  %box = call ptr @ubel_gc_alloc(i64 %bsz, i64 0)
  %bv = getelementptr inbounds %Box, ptr %box, i32 0, i32 0
  store i64 %r, ptr %bv
  %bt = getelementptr inbounds %Box, ptr %box, i32 0, i32 1
  store i64 %n1, ptr %bt
  ret ptr %box
}

; HIGH callback that copies the arena tree out into the GC heap.
define internal ptr @cb_copy(ptr %clo, ptr %node) nounwind {
entry:
  %t = call ptr @copy_expr_gc(ptr %node)
  ret ptr %t
}

; HIGH callback for the iterator pattern: one result per item, plus three
; short-lived GC temporaries so collections happen while MID frames are live.
define internal ptr @cb_item(ptr %clo, ptr %item) nounwind {
entry:
  %kp = getelementptr inbounds %Clo, ptr %clo, i32 0, i32 1
  %k  = load i64, ptr %kp
  %pa = getelementptr inbounds %Item, ptr %item, i32 0, i32 0
  %a  = load i64, ptr %pa
  %pb = getelementptr inbounds %Item, ptr %item, i32 0, i32 1
  %b  = load i64, ptr %pb
  %m  = mul i64 %a, %b
  %r  = add i64 %m, %k
  %g1 = call ptr @ubel_gc_alloc(i64 64, i64 0)
  store i64 %r, ptr %g1
  %g2 = call ptr @ubel_gc_alloc(i64 64, i64 0)
  store i64 %a, ptr %g2
  %g3 = call ptr @ubel_gc_alloc(i64 64, i64 0)
  store i64 %b, ptr %g3
  %bsz = call i64 @Box_size()
  %box = call ptr @ubel_gc_alloc(i64 %bsz, i64 0)
  %bv = getelementptr inbounds %Box, ptr %box, i32 0, i32 0
  store i64 %r, ptr %bv
  ret ptr %box
}

; ========================================================= MID: owns the arena

; MID -> HIGH, only via the closure parameter. The arena dies before return,
; so the result must already be GC-owned (the compiler checks R has no arena refs).
define internal ptr @mid_with_callback(i64 %depth, i64 %seed, ptr %clo) noinline nounwind {
entry:
  %arena = call ptr @ubel_arena_create(i64 0)
  %root  = call ptr @build_expr_arena(ptr %arena, i64 %depth, i64 %seed)
  %fp    = load ptr, ptr %clo
  %r     = call ptr %fp(ptr %clo, ptr %root)
  call void @ubel_arena_destroy(ptr %arena)
  ret ptr %r
}

; Iterator pattern, results kept in an ARENA array. scan = false is UNSAFE
; (a plain arena hides the GC boxes from the collector); scan = true makes
; the arena a root set. Kept in this file to demonstrate and measure both.
define internal i64 @mid_each_arr(i64 %n, ptr %clo, i1 %scan) noinline nounwind {
entry:
  br i1 %scan, label %mk.scan, label %mk.plain
mk.scan:
  %a1 = call ptr @ubel_arena_create_scan(i64 0)
  br label %mk.done
mk.plain:
  %a0 = call ptr @ubel_arena_create(i64 0)
  br label %mk.done
mk.done:
  %arena = phi ptr [ %a1, %mk.scan ], [ %a0, %mk.plain ]
  %psz = ptrtoint ptr getelementptr (ptr, ptr null, i32 1) to i64
  %raw = mul i64 %n, %psz
  %r15 = add i64 %raw, 15
  %rb  = and i64 %r15, -16
  %res = call ptr @arena_bump(ptr %arena, i64 %rb)
  %fp  = load ptr, ptr %clo
  br label %loop
loop:
  %i = phi i64 [ 0, %mk.done ], [ %i.next, %body ]
  %done = icmp eq i64 %i, %n
  br i1 %done, label %sum, label %body
body:
  %isz  = call i64 @Item_asize()
  %item = call ptr @arena_bump(ptr %arena, i64 %isz)
  %ia = getelementptr inbounds %Item, ptr %item, i32 0, i32 0
  store i64 %i, ptr %ia
  %i7  = mul i64 %i, 7
  %ib0 = add i64 %i7, 1
  %ib = getelementptr inbounds %Item, ptr %item, i32 0, i32 1
  store i64 %ib0, ptr %ib
  %box  = call ptr %fp(ptr %clo, ptr %item)
  %slot = getelementptr ptr, ptr %res, i64 %i
  store ptr %box, ptr %slot
  %i.next = add i64 %i, 1
  br label %loop
sum:
  br label %sloop
sloop:
  %j   = phi i64 [ 0, %sum ], [ %j.next, %sbody ]
  %acc = phi i64 [ 0, %sum ], [ %acc.next, %sbody ]
  %sdone = icmp eq i64 %j, %n
  br i1 %sdone, label %exit, label %sbody
sbody:
  %sslot = getelementptr ptr, ptr %res, i64 %j
  %p  = load ptr, ptr %sslot
  %vp = getelementptr inbounds %Box, ptr %p, i32 0, i32 0
  %v  = load i64, ptr %vp
  %acc.next = add i64 %acc, %v
  %j.next = add i64 %j, 1
  br label %sloop
exit:
  call void @ubel_arena_destroy(ptr %arena)
  ret i64 %acc
}

; Iterator pattern, results kept in a GC-owned linked list: the collector sees
; every result through the list head on the stack.
define internal i64 @mid_each_list(i64 %n, ptr %clo) noinline nounwind {
entry:
  %arena = call ptr @ubel_arena_create(i64 0)
  %fp = load ptr, ptr %clo
  br label %loop
loop:
  %i    = phi i64 [ 0, %entry ], [ %i.next, %body ]
  %head = phi ptr [ null, %entry ], [ %node, %body ]
  %done = icmp eq i64 %i, %n
  br i1 %done, label %sum, label %body
body:
  %isz  = call i64 @Item_asize()
  %item = call ptr @arena_bump(ptr %arena, i64 %isz)
  %ia = getelementptr inbounds %Item, ptr %item, i32 0, i32 0
  store i64 %i, ptr %ia
  %i7  = mul i64 %i, 7
  %ib0 = add i64 %i7, 1
  %ib = getelementptr inbounds %Item, ptr %item, i32 0, i32 1
  store i64 %ib0, ptr %ib
  %box = call ptr %fp(ptr %clo, ptr %item)
  %rsz = call i64 @Res_size()
  %rpm = call i64 @Res_ptrmask()
  %node = call ptr @ubel_gc_alloc(i64 %rsz, i64 %rpm)
  %n0 = getelementptr inbounds %Res, ptr %node, i32 0, i32 0
  store ptr %box, ptr %n0
  %n1 = getelementptr inbounds %Res, ptr %node, i32 0, i32 1
  store ptr %head, ptr %n1
  %i.next = add i64 %i, 1
  br label %loop
sum:
  br label %sloop
sloop:
  %cur = phi ptr [ %head, %sum ], [ %next, %sbody ]
  %acc = phi i64 [ 0, %sum ], [ %acc.next, %sbody ]
  %end = icmp eq ptr %cur, null
  br i1 %end, label %exit, label %sbody
sbody:
  %bp = getelementptr inbounds %Res, ptr %cur, i32 0, i32 0
  %b  = load ptr, ptr %bp
  %vp = getelementptr inbounds %Box, ptr %b, i32 0, i32 0
  %v  = load i64, ptr %vp
  %np = getelementptr inbounds %Res, ptr %cur, i32 0, i32 1
  %next = load ptr, ptr %np
  %acc.next = add i64 %acc, %v
  br label %sloop
exit:
  call void @ubel_arena_destroy(ptr %arena)
  ret i64 %acc
}

; ============================================ exported benchmarks, HIGH drivers
; Signature: (depth, reps) -> checksum.

; HIGH -> MID -> HIGH callback: crossing, arena lifecycle, GC-owned result.
define i64 @xcb_callback(i64 %depth, i64 %reps) nounwind {
entry:
  %clo = call ptr @make_closure(ptr @cb_eval, i64 3)
  br label %loop
loop:
  %rep = phi i64 [ 0, %entry ], [ %rep.next, %body ]
  %acc = phi i64 [ 0, %entry ], [ %acc.next, %body ]
  %done = icmp eq i64 %rep, %reps
  br i1 %done, label %exit, label %body
body:
  %box = call ptr @mid_with_callback(i64 %depth, i64 %rep, ptr %clo)
  %bvp = getelementptr inbounds %Box, ptr %box, i32 0, i32 0
  %bv  = load i64, ptr %bvp
  %btp = getelementptr inbounds %Box, ptr %box, i32 0, i32 1
  %bt  = load i64, ptr %btp
  %x   = xor i64 %bv, %bt
  %acc.next = add i64 %acc, %x
  %rep.next = add i64 %rep, 1
  br label %loop
exit:
  %cp = getelementptr inbounds %Clo, ptr %clo, i32 0, i32 2
  %counter = load ptr, ptr %cp
  %np = getelementptr inbounds %Box, ptr %counter, i32 0, i32 0
  %n  = load i64, ptr %np
  %r  = add i64 %acc, %n
  ret i64 %r
}

; HIGH -> MID, the callback copies the arena tree into the GC heap; HIGH then
; evaluates the owned tree AFTER the arena is gone (and its chunk reused).
define i64 @xcb_copyout(i64 %depth, i64 %reps) nounwind {
entry:
  %clo = call ptr @make_closure(ptr @cb_copy, i64 3)
  br label %loop
loop:
  %rep = phi i64 [ 0, %entry ], [ %rep.next, %body ]
  %acc = phi i64 [ 0, %entry ], [ %acc.next, %body ]
  %done = icmp eq i64 %rep, %reps
  br i1 %done, label %exit, label %body
body:
  %tree = call ptr @mid_with_callback(i64 %depth, i64 %rep, ptr %clo)
  %v    = call i64 @eval_expr(ptr %tree)
  %r    = add i64 %v, 3
  %tag  = add i64 %rep, 1
  %x    = xor i64 %r, %tag
  %acc.next = add i64 %acc, %x
  %rep.next = add i64 %rep, 1
  br label %loop
exit:
  %res = add i64 %acc, %reps
  ret i64 %res
}

; ------- baselines with no boundary at all, one per tier, same checksum

define i64 @xcb_mid_only(i64 %depth, i64 %reps) nounwind {
entry:
  br label %loop
loop:
  %rep = phi i64 [ 0, %entry ], [ %rep.next, %body ]
  %acc = phi i64 [ 0, %entry ], [ %acc.next, %body ]
  %done = icmp eq i64 %rep, %reps
  br i1 %done, label %exit, label %body
body:
  %arena = call ptr @ubel_arena_create(i64 0)
  %root  = call ptr @build_expr_arena(ptr %arena, i64 %depth, i64 %rep)
  %v     = call i64 @eval_expr(ptr %root)
  call void @ubel_arena_destroy(ptr %arena)
  %r   = add i64 %v, 3
  %tag = add i64 %rep, 1
  %x   = xor i64 %r, %tag
  %acc.next = add i64 %acc, %x
  %rep.next = add i64 %rep, 1
  br label %loop
exit:
  %res = add i64 %acc, %reps
  ret i64 %res
}

define i64 @xcb_high_only(i64 %depth, i64 %reps) nounwind {
entry:
  br label %loop
loop:
  %rep = phi i64 [ 0, %entry ], [ %rep.next, %body ]
  %acc = phi i64 [ 0, %entry ], [ %acc.next, %body ]
  %done = icmp eq i64 %rep, %reps
  br i1 %done, label %exit, label %body
body:
  %root = call ptr @build_expr_gc(i64 %depth, i64 %rep)
  %v    = call i64 @eval_expr(ptr %root)
  %r    = add i64 %v, 3
  %tag  = add i64 %rep, 1
  %x    = xor i64 %r, %tag
  %acc.next = add i64 %acc, %x
  %rep.next = add i64 %rep, 1
  br label %loop
exit:
  %res = add i64 %acc, %reps
  ret i64 %res
}

define i64 @xcb_low_only(i64 %depth, i64 %reps) nounwind {
entry:
  br label %loop
loop:
  %rep = phi i64 [ 0, %entry ], [ %rep.next, %body ]
  %acc = phi i64 [ 0, %entry ], [ %acc.next, %body ]
  %done = icmp eq i64 %rep, %reps
  br i1 %done, label %exit, label %body
body:
  %root = call ptr @build_expr_low(i64 %depth, i64 %rep)
  %v    = call i64 @eval_expr(ptr %root)
  call void @free_expr_low(ptr %root)
  %r    = add i64 %v, 3
  %tag  = add i64 %rep, 1
  %x    = xor i64 %r, %tag
  %acc.next = add i64 %acc, %x
  %rep.next = add i64 %rep, 1
  br label %loop
exit:
  %res = add i64 %acc, %reps
  ret i64 %res
}

; ============================================== exported: the iterator pattern
; Signature: (n) -> sum over i < n of (i * (7 * i + 1) + 3).

define i64 @xit_arena_plain(i64 %n) nounwind {   ; UNSAFE, hazard demo only
entry:
  %clo = call ptr @make_closure(ptr @cb_item, i64 3)
  %r = call i64 @mid_each_arr(i64 %n, ptr %clo, i1 false)
  ret i64 %r
}

define i64 @xit_arena_scan(i64 %n) nounwind {
entry:
  %clo = call ptr @make_closure(ptr @cb_item, i64 3)
  %r = call i64 @mid_each_arr(i64 %n, ptr %clo, i1 true)
  ret i64 %r
}

define i64 @xit_gc_list(i64 %n) nounwind {
entry:
  %clo = call ptr @make_closure(ptr @cb_item, i64 3)
  %r = call i64 @mid_each_list(i64 %n, ptr %clo)
  ret i64 %r
}

; ============================================= exported: calls into LOW code
; Signature: (n, reps) -> checksum. n must be at most 32 (a 256-byte GC object).

; LOW tier: reads a buffer, allocates nothing, calls nothing in any other tier.
define internal i64 @low_sum(ptr %buf, i64 %n, i64 %salt) noinline nounwind {
entry:
  br label %loop
loop:
  %i   = phi i64 [ 0, %entry ], [ %i.next, %body ]
  %acc = phi i64 [ 0, %entry ], [ %acc.next, %body ]
  %done = icmp eq i64 %i, %n
  br i1 %done, label %exit, label %body
body:
  %p = getelementptr i64, ptr %buf, i64 %i
  %v = load i64, ptr %p
  %t = add i64 %v, %salt
  %acc.next = add i64 %acc, %t
  %i.next = add i64 %i, 1
  br label %loop
exit:
  ret i64 %acc
}

define internal i64 @sum_inl(ptr %buf, i64 %n, i64 %salt) alwaysinline nounwind {
entry:
  br label %loop
loop:
  %i   = phi i64 [ 0, %entry ], [ %i.next, %body ]
  %acc = phi i64 [ 0, %entry ], [ %acc.next, %body ]
  %done = icmp eq i64 %i, %n
  br i1 %done, label %exit, label %body
body:
  %p = getelementptr i64, ptr %buf, i64 %i
  %v = load i64, ptr %p
  %t = add i64 %v, %salt
  %acc.next = add i64 %acc, %t
  %i.next = add i64 %i, 1
  br label %loop
exit:
  ret i64 %acc
}

define internal void @fill_buf(ptr %buf, i64 %n) alwaysinline nounwind {
entry:
  br label %loop
loop:
  %i = phi i64 [ 0, %entry ], [ %i.next, %body ]
  %done = icmp eq i64 %i, %n
  br i1 %done, label %exit, label %body
body:
  %p  = getelementptr i64, ptr %buf, i64 %i
  %i3 = mul i64 %i, 3
  %v  = add i64 %i3, 1
  store i64 %v, ptr %p
  %i.next = add i64 %i, 1
  br label %loop
exit:
  ret void
}

define i64 @xlow_from_high(i64 %n, i64 %reps) nounwind {
entry:
  %bytes = shl i64 %n, 3
  %buf = call ptr @ubel_gc_alloc(i64 %bytes, i64 0)
  call void @fill_buf(ptr %buf, i64 %n)
  br label %loop
loop:
  %rep = phi i64 [ 0, %entry ], [ %rep.next, %body ]
  %acc = phi i64 [ 0, %entry ], [ %acc.next, %body ]
  %done = icmp eq i64 %rep, %reps
  br i1 %done, label %exit, label %body
body:
  %s = call i64 @low_sum(ptr %buf, i64 %n, i64 %rep)
  %x = xor i64 %s, %rep
  %acc.next = add i64 %acc, %x
  %rep.next = add i64 %rep, 1
  br label %loop
exit:
  ret i64 %acc
}

define i64 @xlow_from_mid(i64 %n, i64 %reps) nounwind {
entry:
  %arena = call ptr @ubel_arena_create(i64 0)
  %bytes = shl i64 %n, 3
  %rb0 = add i64 %bytes, 15
  %rb  = and i64 %rb0, -16
  %buf = call ptr @arena_bump(ptr %arena, i64 %rb)
  call void @fill_buf(ptr %buf, i64 %n)
  br label %loop
loop:
  %rep = phi i64 [ 0, %entry ], [ %rep.next, %body ]
  %acc = phi i64 [ 0, %entry ], [ %acc.next, %body ]
  %done = icmp eq i64 %rep, %reps
  br i1 %done, label %exit, label %body
body:
  %s = call i64 @low_sum(ptr %buf, i64 %n, i64 %rep)
  %x = xor i64 %s, %rep
  %acc.next = add i64 %acc, %x
  %rep.next = add i64 %rep, 1
  br label %loop
exit:
  call void @ubel_arena_destroy(ptr %arena)
  ret i64 %acc
}

; Baseline: same loop, summation inlined, so no call boundary.
define i64 @xlow_inline(i64 %n, i64 %reps) nounwind {
entry:
  %bytes = shl i64 %n, 3
  %bp    = inttoptr i64 %bytes to ptr
  %msz   = ptrtoint ptr %bp to %size_t
  %buf   = call ptr @malloc(%size_t %msz)
  call void @fill_buf(ptr %buf, i64 %n)
  br label %loop
loop:
  %rep = phi i64 [ 0, %entry ], [ %rep.next, %body ]
  %acc = phi i64 [ 0, %entry ], [ %acc.next, %body ]
  %done = icmp eq i64 %rep, %reps
  br i1 %done, label %exit, label %body
body:
  %s = call i64 @sum_inl(ptr %buf, i64 %n, i64 %rep)
  %x = xor i64 %s, %rep
  %acc.next = add i64 %acc, %x
  %rep.next = add i64 %rep, 1
  br label %loop
exit:
  call void @free(ptr %buf)
  ret i64 %acc
}
