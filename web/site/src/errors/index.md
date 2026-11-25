# Errors and diagnostics

The compiler reports every problem as a **diagnostic**: a stable code, a
one-line message, the exact source span, and where useful a note and a `help:`
line. This section documents each code: what triggers it, a small program that
does, the exact output, and how to fix it.

```text
>>> error[NAME-001]: undefined name `count`
  --> 2:17
    |
  2 |     let total = count + 1
    |                 ^^^^^
<<<
```

- **`NAME-001`** is the code. It never changes meaning, so you can search for it,
  link to it, and match on it in tooling (`ubel --json check` and the language
  server both carry it). The playground turns it into a link to this page.
- The **family** is the part before the dash and says which part of the compiler
  found the problem.
- The **span** (`2:17` is line 2, column 17) and the caret line show the exact text.

## Families

| Family | Stage | Covers | Page |
|--------|-------|--------|------|
| `LEX-`  | lex   | splitting text into tokens | [Lexical errors](./lex.md) |
| `PARSE-` | parse | tokens that do not form valid constructs | [Parse errors](./parse.md) |
| `NAME-` | sema  | names, scopes, imports, constants | [Name resolution](./name.md) |
| `TYPE-` | sema  | types, enums, formatting, traits and bounds | [Type errors](./type.md) |
| `TIER-` | sema  | the tier call matrix, arenas, pools, ownership wrappers | [Tier errors](./tier.md) |
| `BORROW-` `MOVE-` `LIFETIME-` | sema | LOW-tier borrows, moves and lifetimes | [Ownership errors](./ownership.md) |

Compilation stops at the first stage that reports an error. A file with a parse
error therefore shows no type errors until the parse error is fixed, and the
summary line says which stage stopped (`stopped at sema`).

## Every code

An asterisk marks an entry with no verified example yet: the compiler defines the
code, but it could not be reproduced from ordinary source when the page was
written. `TIER-002`, `TIER-003` and `TIER-007` are retired and their numbers are not reused.

| Code | Name |
|------|------|
| [BORROW-001](./ownership.md#borrow-001) | ConflictingAccessWhileBorrowed |
| [LEX-001](./lex.md#lex-001) | UnexpectedChar |
| [LEX-002](./lex.md#lex-002) | UnterminatedString \* |
| [LEX-003](./lex.md#lex-003) | UnterminatedBlockComment |
| [LEX-004](./lex.md#lex-004) | InvalidNumber \* |
| [LEX-005](./lex.md#lex-005) | InvalidEscape \* |
| [LEX-006](./lex.md#lex-006) | InvalidInterpolation |
| [LEX-007](./lex.md#lex-007) | InvalidCharLiteral \* |
| [LIFETIME-001](./ownership.md#lifetime-001) | UndeclaredLifetime |
| [LIFETIME-002](./ownership.md#lifetime-002) | DuplicateLifetimeParam \* |
| [LIFETIME-003](./ownership.md#lifetime-003) | OutlivesCycle |
| [LIFETIME-004](./ownership.md#lifetime-004) | BoundaryTooShort |
| [LIFETIME-005](./ownership.md#lifetime-005) | OutlivesConstraintViolated |
| [LIFETIME-006](./ownership.md#lifetime-006) | NonLocalBoundaryValue |
| [MOVE-001](./ownership.md#move-001) | UseAfterMove |
| [NAME-001](./name.md#name-001) | UndefinedName |
| [NAME-002](./name.md#name-002) | DuplicateDefinition |
| [NAME-003](./name.md#name-003) | UnresolvedImport |
| [NAME-004](./name.md#name-004) | UnresolvedPathSegment \* |
| [NAME-005](./name.md#name-005) | SelfOutsideMethod |
| [NAME-006](./name.md#name-006) | UnresolvedTypeParam \* |
| [NAME-007](./name.md#name-007) | AssignToConst |
| [NAME-008](./name.md#name-008) | StaticInConst |
| [NAME-009](./name.md#name-009) | InitializationCycle |
| [PARSE-001](./parse.md#parse-001) | UnexpectedToken |
| [PARSE-002](./parse.md#parse-002) | UnexpectedEof |
| [PARSE-003](./parse.md#parse-003) | UnclosedDelimiter |
| [PARSE-004](./parse.md#parse-004) | IllegalInContext |
| [PARSE-005](./parse.md#parse-005) | Raw \* |
| [PARSE-006](./parse.md#parse-006) | NestingTooDeep |
| [TIER-001](./tier.md#tier-001) | ArenaInWrongTier |
| [TIER-004](./tier.md#tier-004) | ArenaRefEscapesBoundary |
| [TIER-005](./tier.md#tier-005) | IllegalTierCall |
| [TIER-006](./tier.md#tier-006) | MidReturnContainsArenaRef \* |
| [TIER-008](./tier.md#tier-008) | MethodInWrongTier |
| [TIER-009](./tier.md#tier-009) | CollectionConstructionInLowTier |
| [TIER-010](./tier.md#tier-010) | PoolInWrongTier |
| [TIER-011](./tier.md#tier-011) | PoolRefEscapesBoundary |
| [TIER-012](./tier.md#tier-012) | MidReturnContainsPoolRef \* |
| [TIER-013](./tier.md#tier-013) | PoolConstructedOutsideBlock |
| [TIER-014](./tier.md#tier-014) | OwnershipWrapperOutsideLowTier |
| [TIER-015](./tier.md#tier-015) | StaticAccessOutsideHigh |
| [TIER-016](./tier.md#tier-016) | TraitMethodTierMismatch |
| [TIER-017](./tier.md#tier-017) | LowCallsUnknownClosure |
| [TYPE-101](./type.md#type-101) | TypeMismatch |
| [TYPE-102](./type.md#type-102) | ArgumentCountMismatch |
| [TYPE-103](./type.md#type-103) | NoSuchField |
| [TYPE-104](./type.md#type-104) | NoSuchMethod |
| [TYPE-105](./type.md#type-105) | TryOnNonFallible \* |
| [TYPE-106](./type.md#type-106) | AwaitOnNonTask |
| [TYPE-107](./type.md#type-107) | CannotInferType \* |
| [TYPE-108](./type.md#type-108) | GenericArgCountMismatch |
| [TYPE-109](./type.md#type-109) | UnknownVariant |
| [TYPE-110](./type.md#type-110) | VariantArityMismatch |
| [TYPE-111](./type.md#type-111) | NonExhaustiveMatch |
| [TYPE-112](./type.md#type-112) | MixedDiscriminantAndPayload |
| [TYPE-113](./type.md#type-113) | InlineListCapacityNotLiteral |
| [TYPE-114](./type.md#type-114) | DerefOnNonReference |
| [TYPE-115](./type.md#type-115) | InvalidFormatSpec |
| [TYPE-116](./type.md#type-116) | UnknownDeriveTrait |
| [TYPE-117](./type.md#type-117) | DeriveRequiresOther |
| [TYPE-118](./type.md#type-118) | TypeNotOrderable |
| [TYPE-119](./type.md#type-119) | AlternateFormatWithoutBase |
| [TYPE-120](./type.md#type-120) | IntLiteralOutOfRange |
| [TYPE-121](./type.md#type-121) | TypeAliasCycle |
| [TYPE-122](./type.md#type-122) | NotATrait |
| [TYPE-123](./type.md#type-123) | TraitMethodMissing |
| [TYPE-124](./type.md#type-124) | UnknownTraitMethod |
| [TYPE-125](./type.md#type-125) | TraitMethodSignatureMismatch |
| [TYPE-126](./type.md#type-126) | UnsatisfiedBound |
| [TYPE-127](./type.md#type-127) | AmbiguousTraitMethod |
| [TYPE-128](./type.md#type-128) | OverlappingImpl |
| [TYPE-129](./type.md#type-129) | UnsupportedTraitFeature |
| [TYPE-130](./type.md#type-130) | MethodOnUnboundedParam |
| [TYPE-131](./type.md#type-131) | OperatorNeedsBound |
| [TYPE-132](./type.md#type-132) | MissingSupertraitImpl |
| [TYPE-133](./type.md#type-133) | SupertraitCycle |
| [TYPE-134](./type.md#type-134) | ConflictingBounds |
| [TYPE-135](./type.md#type-135) | NotIterable |
| [TYPE-136](./type.md#type-136) | OperatorNotImplemented |
| [TYPE-137](./type.md#type-137) | NotIndexable |
| [TYPE-138](./type.md#type-138) | NotObjectSafe |
| [TYPE-139](./type.md#type-139) | OperatorNotSupported |
| [TYPE-140](./type.md#type-140) | NotCallable |
| [TYPE-141](./type.md#type-141) | TupleIndexOutOfRange |
| [TYPE-142](./type.md#type-142) | NotAStruct |
| [TYPE-143](./type.md#type-143) | AwaitOutsideAsync |
| [TYPE-144](./type.md#type-144) | AsyncReturnNotTask |
| [TYPE-145](./type.md#type-145) | MarkerImplField |
| [TYPE-146](./type.md#type-146) | UnsafeImplNotMarker |

## For contributors

Adding a diagnostic means adding its entry to the registry in
`docs/DIAGNOSTICS_RULES.md` and its entry to the matching page here, with an
example that the compiler really rejects. Write the example as a fenced `ubel`
block preceded by `<!-- ubel-test: error CODE -->`; CI
(`scripts/check_doc_examples.py`) then fails if the example stops producing
exactly that code. See `docs/SITE_DOCS.md`.
