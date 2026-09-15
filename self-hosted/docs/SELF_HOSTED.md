This documentation outlines the purpose, vision, and architectural roadmap for the Ubel Stratum Self-Hosted Compiler Initiative (/selfhosted/README.md).
Ubel Stratum Self-Hosted Compiler Core
What is the Self-Hosted Initiative?
Self-hosting (bootstrapping) is the process of writing the Ubel Stratum compiler pipeline entirely in Ubel Stratum (.ubl) itself. Rather than relying permanently on an external host toolchain (such as Rust or C#), the core compiler components—lexical analyzer, parser, AST synthesis, type checker, and byte/code emission—are implemented natively using Ubel syntax and semantics.
       +-------------------------------------------------------+
       |             Self-Hosted Ubel Pipeline                 |
       |                                                       |
       |  [ Source Code (.ubl) ]                               |
       |           |                                           |
       |           v                                           |
       |    Tokenizer (tokenizer.ubl)                          |
       |           |                                           |
       |           v                                           |
       |    Parser & AST Builder (parser.ubl)                  |
       |           |                                           |
       |           v                                           |
       |    Type Checker & IR Optimizer                        |
       |           |                                           |
       |           v                                           |
       |    Code Generator / Bytecode Emitter                  |
       +-------------------------------------------------------+

Why Self-Host? Core Objectives
 * Language Battle-Testing (Dogfooding): Building a non-trivial system like a compiler forces Ubel's syntax, stdlib collections (List, Map), and memory models to prove their real-world ergonomics and stability.
 * Validating Multi-Tier Memory Semantics: The self-hosted compiler serves as the primary benchmark for Ubel's memory tiers:
   * Span<T>: Utilized in the lexer for zero-allocation byte slicing over raw text buffers.
   * Unique<T>: Utilized for owned AST nodes and local variable environments.
   * Shared<T>: Utilized for cross-referenced symbol tables and shared type registries.
 * Proving Grammar Correctness: Successfully compiling recursive language structures (e.g., nested control blocks, match statements, expression trees) validates that Ubel's grammar spec is free of ambiguous parsing rules.
 * Eliminating Host Dependencies: Creates an independent, self-sustaining development loop where new Ubel language features can immediately be used to enhance the compiler itself.
 * Performance & Safety Verification: Exposes compiler bottlenecks, stack overflow risks in recursion, type-caster edge cases, and memory leaks before third-party developers encounter them.
Bootstrap Strategy & Phased Rollout
| Phase | Milestone | Primary Focus | Status |
|---|---|---|---|
| Phase 1 | Lexical Analyzer | Tokenizing ASCII streams into Token variants using explicit type casts and boundary checks. | Active |
| Phase 2 | Syntax Parser & AST | Parsing tokens into structured nodes (JsonProperty, ParseResult) and verifying control structures. | Active |
| Phase 3 | Type Checker | Validating type consistency, enforcing explicit casts (as double), and symbol table resolution. | Planned |
| Phase 4 | Self-Compilation | Compiling tokenizer.ubl and parser.ubl using the self-hosted executable itself. | Planned |
