; crates/llvm_poc/bench/tiers.ll
;
; Hand-written LLVM IR (opaque pointers, LLVM 15+), written in the shape a
; tier-aware Ubel backend would emit. No compiler produced this file.
;
;   HIGH  -> every allocation is a call into the GC runtime
;   MID   -> `with arena { }` = create / bump / destroy; the bump fast path
;            is inlined (load cur, add size, compare end, store cur)
;   LOW   -> plain malloc / free, freed explicitly
;
; Every workload has a tier-independent walk/sum, so all three tiers must
; return the same checksum. The harness checks that.
;
; Deliberately no `target datalayout` / `target triple`: clang fills in the
; target's, so the same file builds for x86_64, aarch64 and 32-bit targets.
; Object sizes and pointer masks are derived from the data layout below, the
; way a backend would, instead of being hard-coded for 64-bit pointers.

; run_bench.sh rewrites the next line to i32 when the target's size_t is 32-bit.
%size_t = type i64

%Arena = type { ptr, ptr, ptr, i64 }       ; cur, end, chunks, chunk_size  (== ubel_arena)
%Node  = type { i64, ptr, i64, i64 }       ; val, next, a, b          32 bytes, ptr at word 1
%TNode = type { i64, ptr, ptr }            ; val, left, right         24 bytes, ptrs at words 1,2

declare ptr  @ubel_gc_alloc(i64, i64)
declare ptr  @ubel_arena_create(i64)
declare ptr  @ubel_arena_grow(ptr, i64)
declare void @ubel_arena_destroy(ptr)
declare ptr  @malloc(%size_t)
declare void @free(ptr)

!0 = !{!"branch_weights", i32 2000, i32 1}


; ===================================== layout-derived sizes and pointer masks

define internal i64 @node_size() alwaysinline nounwind readnone {
entry:
  ret i64 ptrtoint (ptr getelementptr (%Node, ptr null, i32 1) to i64)
}
define internal i64 @tnode_size() alwaysinline nounwind readnone {
entry:
  ret i64 ptrtoint (ptr getelementptr (%TNode, ptr null, i32 1) to i64)
}
; malloc takes a size_t, which is i32 on 32-bit targets.
define internal %size_t @node_msize() alwaysinline nounwind readnone {
entry:
  ret %size_t ptrtoint (ptr getelementptr (%Node, ptr null, i32 1) to %size_t)
}
define internal %size_t @tnode_msize() alwaysinline nounwind readnone {
entry:
  ret %size_t ptrtoint (ptr getelementptr (%TNode, ptr null, i32 1) to %size_t)
}
; Arena requests are rounded up to a multiple of 16.
define internal i64 @node_asize() alwaysinline nounwind readnone {
entry:
  %s = call i64 @node_size()
  %a = add i64 %s, 15
  %r = and i64 %a, -16
  ret i64 %r
}
define internal i64 @tnode_asize() alwaysinline nounwind readnone {
entry:
  %s = call i64 @tnode_size()
  %a = add i64 %s, 15
  %r = and i64 %a, -16
  ret i64 %r
}
; Bit i set = the pointer-sized word at byte offset i * sizeof(ptr) holds a pointer.
define internal i64 @node_ptrmask() alwaysinline nounwind readnone {
entry:
  %off = ptrtoint ptr getelementptr (%Node, ptr null, i32 0, i32 1) to i64
  %psz = ptrtoint ptr getelementptr (ptr, ptr null, i32 1) to i64
  %idx = udiv i64 %off, %psz
  %m   = shl i64 1, %idx
  ret i64 %m
}
define internal i64 @tnode_ptrmask() alwaysinline nounwind readnone {
entry:
  %psz  = ptrtoint ptr getelementptr (ptr, ptr null, i32 1) to i64
  %off1 = ptrtoint ptr getelementptr (%TNode, ptr null, i32 0, i32 1) to i64
  %off2 = ptrtoint ptr getelementptr (%TNode, ptr null, i32 0, i32 2) to i64
  %i1   = udiv i64 %off1, %psz
  %i2   = udiv i64 %off2, %psz
  %m1   = shl i64 1, %i1
  %m2   = shl i64 1, %i2
  %m    = or i64 %m1, %m2
  ret i64 %m
}

; ============================================================ shared helpers

; MID-tier bump allocation. `size` must be a multiple of 16.
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

define internal void @node_init(ptr %n, i64 %val, ptr %next, i64 %a, i64 %b) alwaysinline nounwind {
entry:
  %p0 = getelementptr inbounds %Node, ptr %n, i32 0, i32 0
  store i64 %val, ptr %p0
  %p1 = getelementptr inbounds %Node, ptr %n, i32 0, i32 1
  store ptr %next, ptr %p1
  %p2 = getelementptr inbounds %Node, ptr %n, i32 0, i32 2
  store i64 %a, ptr %p2
  %p3 = getelementptr inbounds %Node, ptr %n, i32 0, i32 3
  store i64 %b, ptr %p3
  ret void
}

define internal void @tnode_init(ptr %n, i64 %val, ptr %l, ptr %r) alwaysinline nounwind {
entry:
  %p0 = getelementptr inbounds %TNode, ptr %n, i32 0, i32 0
  store i64 %val, ptr %p0
  %p1 = getelementptr inbounds %TNode, ptr %n, i32 0, i32 1
  store ptr %l, ptr %p1
  %p2 = getelementptr inbounds %TNode, ptr %n, i32 0, i32 2
  store ptr %r, ptr %p2
  ret void
}

define internal i64 @walk_list(ptr %head) nounwind {
entry:
  br label %loop
loop:
  %cur = phi ptr [ %head, %entry ], [ %next, %body ]
  %acc = phi i64 [ 0, %entry ], [ %acc.next, %body ]
  %end = icmp eq ptr %cur, null
  br i1 %end, label %exit, label %body
body:
  %pv   = getelementptr inbounds %Node, ptr %cur, i32 0, i32 0
  %v    = load i64, ptr %pv
  %pn   = getelementptr inbounds %Node, ptr %cur, i32 0, i32 1
  %next = load ptr, ptr %pn
  %pa   = getelementptr inbounds %Node, ptr %cur, i32 0, i32 2
  %a    = load i64, ptr %pa
  %pb   = getelementptr inbounds %Node, ptr %cur, i32 0, i32 3
  %b    = load i64, ptr %pb
  %t0   = add i64 %v, %a
  %t1   = add i64 %t0, %b
  %acc.next = add i64 %acc, %t1
  br label %loop
exit:
  ret i64 %acc
}

define internal i64 @tree_sum(ptr %t) nounwind {
entry:
  %isnull = icmp eq ptr %t, null
  br i1 %isnull, label %zero, label %rec
zero:
  ret i64 0
rec:
  %pv = getelementptr inbounds %TNode, ptr %t, i32 0, i32 0
  %v  = load i64, ptr %pv
  %pl = getelementptr inbounds %TNode, ptr %t, i32 0, i32 1
  %l  = load ptr, ptr %pl
  %pr = getelementptr inbounds %TNode, ptr %t, i32 0, i32 2
  %r  = load ptr, ptr %pr
  %sl = call i64 @tree_sum(ptr %l)
  %sr = call i64 @tree_sum(ptr %r)
  %s0 = add i64 %v, %sl
  %s1 = add i64 %s0, %sr
  ret i64 %s1
}

; ====================================================================== HIGH

define internal ptr @build_list_high(i64 %n, i64 %seed) nounwind {
entry:
  br label %loop
loop:
  %i    = phi i64 [ 0, %entry ], [ %i.next, %body ]
  %head = phi ptr [ null, %entry ], [ %node, %body ]
  %done = icmp eq i64 %i, %n
  br i1 %done, label %exit, label %body
body:
  %sz   = call i64 @node_size()
  %pm   = call i64 @node_ptrmask()
  %node = call ptr @ubel_gc_alloc(i64 %sz, i64 %pm)
  %val  = xor i64 %i, %seed
  %a    = mul i64 %i, 3
  call void @node_init(ptr %node, i64 %val, ptr %head, i64 %a, i64 %seed)
  %i.next = add i64 %i, 1
  br label %loop
exit:
  ret ptr %head
}

define internal ptr @tree_build_high(i64 %depth, i64 %seed) nounwind {
entry:
  %sz   = call i64 @tnode_size()
  %pm   = call i64 @tnode_ptrmask()
  %node = call ptr @ubel_gc_alloc(i64 %sz, i64 %pm)
  %leaf = icmp eq i64 %depth, 0
  br i1 %leaf, label %leaf.bb, label %rec
leaf.bb:
  call void @tnode_init(ptr %node, i64 %seed, ptr null, ptr null)
  ret ptr %node
rec:
  %d1 = sub i64 %depth, 1
  %s2 = shl i64 %seed, 1
  %l  = call ptr @tree_build_high(i64 %d1, i64 %s2)
  %s3 = or i64 %s2, 1
  %r  = call ptr @tree_build_high(i64 %d1, i64 %s3)
  call void @tnode_init(ptr %node, i64 %seed, ptr %l, ptr %r)
  ret ptr %node
}

define i64 @churn_high(i64 %frames, i64 %per_frame, i64 %live) nounwind {
entry:
  %livehead = call ptr @build_list_high(i64 %live, i64 7)
  br label %loop
loop:
  %f   = phi i64 [ 0, %entry ], [ %f.next, %body ]
  %acc = phi i64 [ 0, %entry ], [ %acc.next, %body ]
  %done = icmp eq i64 %f, %frames
  br i1 %done, label %exit, label %body
body:
  %head = call ptr @build_list_high(i64 %per_frame, i64 %f)
  %s    = call i64 @walk_list(ptr %head)
  %acc.next = add i64 %acc, %s
  %f.next   = add i64 %f, 1
  br label %loop
exit:
  %ls = call i64 @walk_list(ptr %livehead)
  %r  = add i64 %acc, %ls
  ret i64 %r
}

define i64 @tree_high(i64 %depth, i64 %reps) nounwind {
entry:
  br label %loop
loop:
  %rep = phi i64 [ 0, %entry ], [ %rep.next, %body ]
  %acc = phi i64 [ 0, %entry ], [ %acc.next, %body ]
  %done = icmp eq i64 %rep, %reps
  br i1 %done, label %exit, label %body
body:
  %t = call ptr @tree_build_high(i64 %depth, i64 %rep)
  %s = call i64 @tree_sum(ptr %t)
  %acc.next = add i64 %acc, %s
  %rep.next = add i64 %rep, 1
  br label %loop
exit:
  ret i64 %acc
}

; ======================================================================= MID

define internal ptr @build_list_mid(ptr %arena, i64 %n, i64 %seed) nounwind {
entry:
  br label %loop
loop:
  %i    = phi i64 [ 0, %entry ], [ %i.next, %body ]
  %head = phi ptr [ null, %entry ], [ %node, %body ]
  %done = icmp eq i64 %i, %n
  br i1 %done, label %exit, label %body
body:
  %sz   = call i64 @node_asize()
  %node = call ptr @arena_bump(ptr %arena, i64 %sz)
  %val  = xor i64 %i, %seed
  %a    = mul i64 %i, 3
  call void @node_init(ptr %node, i64 %val, ptr %head, i64 %a, i64 %seed)
  %i.next = add i64 %i, 1
  br label %loop
exit:
  ret ptr %head
}

define internal ptr @tree_build_mid(ptr %arena, i64 %depth, i64 %seed) nounwind {
entry:
  %sz   = call i64 @tnode_asize()
  %node = call ptr @arena_bump(ptr %arena, i64 %sz)
  %leaf = icmp eq i64 %depth, 0
  br i1 %leaf, label %leaf.bb, label %rec
leaf.bb:
  call void @tnode_init(ptr %node, i64 %seed, ptr null, ptr null)
  ret ptr %node
rec:
  %d1 = sub i64 %depth, 1
  %s2 = shl i64 %seed, 1
  %l  = call ptr @tree_build_mid(ptr %arena, i64 %d1, i64 %s2)
  %s3 = or i64 %s2, 1
  %r  = call ptr @tree_build_mid(ptr %arena, i64 %d1, i64 %s3)
  call void @tnode_init(ptr %node, i64 %seed, ptr %l, ptr %r)
  ret ptr %node
}

define i64 @churn_mid(i64 %frames, i64 %per_frame, i64 %live) nounwind {
entry:
  %outer    = call ptr @ubel_arena_create(i64 0)
  %livehead = call ptr @build_list_mid(ptr %outer, i64 %live, i64 7)
  br label %loop
loop:
  %f   = phi i64 [ 0, %entry ], [ %f.next, %body ]
  %acc = phi i64 [ 0, %entry ], [ %acc.next, %body ]
  %done = icmp eq i64 %f, %frames
  br i1 %done, label %exit, label %body
body:
  %fa   = call ptr @ubel_arena_create(i64 0)           ; with arena { ... } per frame
  %head = call ptr @build_list_mid(ptr %fa, i64 %per_frame, i64 %f)
  %s    = call i64 @walk_list(ptr %head)
  call void @ubel_arena_destroy(ptr %fa)
  %acc.next = add i64 %acc, %s
  %f.next   = add i64 %f, 1
  br label %loop
exit:
  %ls = call i64 @walk_list(ptr %livehead)
  call void @ubel_arena_destroy(ptr %outer)
  %r  = add i64 %acc, %ls
  ret i64 %r
}

define i64 @tree_mid(i64 %depth, i64 %reps) nounwind {
entry:
  br label %loop
loop:
  %rep = phi i64 [ 0, %entry ], [ %rep.next, %body ]
  %acc = phi i64 [ 0, %entry ], [ %acc.next, %body ]
  %done = icmp eq i64 %rep, %reps
  br i1 %done, label %exit, label %body
body:
  %arena = call ptr @ubel_arena_create(i64 0)
  %t = call ptr @tree_build_mid(ptr %arena, i64 %depth, i64 %rep)
  %s = call i64 @tree_sum(ptr %t)
  call void @ubel_arena_destroy(ptr %arena)
  %acc.next = add i64 %acc, %s
  %rep.next = add i64 %rep, 1
  br label %loop
exit:
  ret i64 %acc
}

; ======================================================================= LOW

define internal ptr @build_list_low(i64 %n, i64 %seed) nounwind {
entry:
  br label %loop
loop:
  %i    = phi i64 [ 0, %entry ], [ %i.next, %body ]
  %head = phi ptr [ null, %entry ], [ %node, %body ]
  %done = icmp eq i64 %i, %n
  br i1 %done, label %exit, label %body
body:
  %sz   = call %size_t @node_msize()
  %node = call ptr @malloc(%size_t %sz)
  %val  = xor i64 %i, %seed
  %a    = mul i64 %i, 3
  call void @node_init(ptr %node, i64 %val, ptr %head, i64 %a, i64 %seed)
  %i.next = add i64 %i, 1
  br label %loop
exit:
  ret ptr %head
}

define internal void @free_list(ptr %head) nounwind {
entry:
  br label %loop
loop:
  %cur = phi ptr [ %head, %entry ], [ %next, %body ]
  %end = icmp eq ptr %cur, null
  br i1 %end, label %exit, label %body
body:
  %pn   = getelementptr inbounds %Node, ptr %cur, i32 0, i32 1
  %next = load ptr, ptr %pn
  call void @free(ptr %cur)
  br label %loop
exit:
  ret void
}

define internal ptr @tree_build_low(i64 %depth, i64 %seed) nounwind {
entry:
  %sz   = call %size_t @tnode_msize()
  %node = call ptr @malloc(%size_t %sz)
  %leaf = icmp eq i64 %depth, 0
  br i1 %leaf, label %leaf.bb, label %rec
leaf.bb:
  call void @tnode_init(ptr %node, i64 %seed, ptr null, ptr null)
  ret ptr %node
rec:
  %d1 = sub i64 %depth, 1
  %s2 = shl i64 %seed, 1
  %l  = call ptr @tree_build_low(i64 %d1, i64 %s2)
  %s3 = or i64 %s2, 1
  %r  = call ptr @tree_build_low(i64 %d1, i64 %s3)
  call void @tnode_init(ptr %node, i64 %seed, ptr %l, ptr %r)
  ret ptr %node
}

define internal void @tree_free(ptr %t) nounwind {
entry:
  %isnull = icmp eq ptr %t, null
  br i1 %isnull, label %done, label %rec
done:
  ret void
rec:
  %pl = getelementptr inbounds %TNode, ptr %t, i32 0, i32 1
  %l  = load ptr, ptr %pl
  %pr = getelementptr inbounds %TNode, ptr %t, i32 0, i32 2
  %r  = load ptr, ptr %pr
  call void @tree_free(ptr %l)
  call void @tree_free(ptr %r)
  call void @free(ptr %t)
  ret void
}

define i64 @churn_low(i64 %frames, i64 %per_frame, i64 %live) nounwind {
entry:
  %livehead = call ptr @build_list_low(i64 %live, i64 7)
  br label %loop
loop:
  %f   = phi i64 [ 0, %entry ], [ %f.next, %body ]
  %acc = phi i64 [ 0, %entry ], [ %acc.next, %body ]
  %done = icmp eq i64 %f, %frames
  br i1 %done, label %exit, label %body
body:
  %head = call ptr @build_list_low(i64 %per_frame, i64 %f)
  %s    = call i64 @walk_list(ptr %head)
  call void @free_list(ptr %head)
  %acc.next = add i64 %acc, %s
  %f.next   = add i64 %f, 1
  br label %loop
exit:
  %ls = call i64 @walk_list(ptr %livehead)
  call void @free_list(ptr %livehead)
  %r  = add i64 %acc, %ls
  ret i64 %r
}

define i64 @tree_low(i64 %depth, i64 %reps) nounwind {
entry:
  br label %loop
loop:
  %rep = phi i64 [ 0, %entry ], [ %rep.next, %body ]
  %acc = phi i64 [ 0, %entry ], [ %acc.next, %body ]
  %done = icmp eq i64 %rep, %reps
  br i1 %done, label %exit, label %body
body:
  %t = call ptr @tree_build_low(i64 %depth, i64 %rep)
  %s = call i64 @tree_sum(ptr %t)
  call void @tree_free(ptr %t)
  %acc.next = add i64 %acc, %s
  %rep.next = add i64 %rep, 1
  br label %loop
exit:
  ret i64 %acc
}

; =============================================== tier-independent baseline

; No allocation at all: shows the tier choice is irrelevant here, and that
; the harness measures only what the workload does.
define i64 @arith(i64 %n) nounwind {
entry:
  br label %loop
loop:
  %i   = phi i64 [ 0, %entry ], [ %i.next, %body ]
  %x   = phi i64 [ 88172645463325252, %entry ], [ %x.next, %body ]
  %acc = phi i64 [ 0, %entry ], [ %acc.next, %body ]
  %done = icmp eq i64 %i, %n
  br i1 %done, label %exit, label %body
body:
  %m  = mul i64 %x, 6364136223846793005
  %x.next = add i64 %m, 1442695040888963407
  %sh = lshr i64 %x.next, 33
  %acc.next = xor i64 %acc, %sh
  %i.next = add i64 %i, 1
  br label %loop
exit:
  ret i64 %acc
}
