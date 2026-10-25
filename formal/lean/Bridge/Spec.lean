/-
Copyright (c) 2026 Martin-PB. All rights reserved.
See the repository root for the applicable license.

Git ↔ Pijul bridge — formal specification layers.

This file specifies contracts before choosing an import algorithm. It does NOT
claim to model the full Pijul patch calculus yet. The abstract Pijul interface
below is an explicit proof boundary, not a theorem about the upstream CLI. Do
not add axioms that simply assert the desired end-to-end correctness property.
-/

import Std

namespace PijulBridge

/-! ## 1. Minimal Git object model -/

abbrev Oid := Nat
abbrev Path := Nat
abbrev BlobId := Nat

/-- A tree is modeled extensionally as a path-to-blob partial map.
    This abstracts Git tree-object encoding and object hashes. -/
abbrev GitTree := Path → Option BlobId

/-- Extensional equality of materialized file trees. This intentionally says
    nothing about commit ancestry, change identity, or conflict semantics. -/
def TreeEq (a b : GitTree) : Prop := ∀ path, a path = b path

theorem treeEq_refl (tree : GitTree) : TreeEq tree tree := by
  intro path
  rfl

theorem treeEq_symm {a b : GitTree} (h : TreeEq a b) : TreeEq b a := by
  intro path
  exact (h path).symm

theorem treeEq_trans {a b c : GitTree}
    (hab : TreeEq a b) (hbc : TreeEq b c) : TreeEq a c := by
  intro path
  exact Eq.trans (hab path) (hbc path)

structure GitCommit where
  oid     : Oid
  parents : List Oid
  tree    : GitTree
  author  : Nat
  message : Nat
  deriving Inhabited

/-- A history is a finite, well-founded portion of the Git commit DAG.
    rank is a certificate used to rule out parent cycles: every parent has
    strictly smaller rank than its child. Two commits with the same tree
    remain distinct commits. -/
structure GitHistory where
  commit : Oid → Option GitCommit
  rank   : Oid → Nat
  parentExists : ∀ child c, commit child = some c →
    ∀ parent, parent ∈ c.parents → ∃ p, commit parent = some p
  parentRank : ∀ child c, commit child = some c →
    ∀ parent, parent ∈ c.parents → rank parent < rank child
  oidMatches : ∀ oid c, commit oid = some c → c.oid = oid
  parentsNoDup : ∀ oid c, commit oid = some c → c.parents.Nodup

namespace GitHistory

def ParentEdge (h : GitHistory) (parent child : Oid) : Prop :=
  ∃ c, h.commit child = some c ∧ parent ∈ c.parents

/-- Reflexive transitive ancestry: an ancestor is the commit itself or a
    parent reached by following zero or more parent edges backwards. -/
inductive Ancestor (h : GitHistory) : Oid → Oid → Prop
  | refl (x : Oid) : Ancestor h x x
  | step {ancestor parent child : Oid} :
      Ancestor h ancestor parent →
      ParentEdge h parent child →
      Ancestor h ancestor child

theorem parent_is_ancestor (h : GitHistory) {p c : Oid}
    (edge : ParentEdge h p c) : Ancestor h p c :=
  Ancestor.step (Ancestor.refl p) edge

theorem ancestry_transitive (h : GitHistory) {a b c : Oid}
    (hab : Ancestor h a b) (hbc : Ancestor h b c) : Ancestor h a c := by
  induction hbc with
  | refl => exact hab
  | step hbp edge ih =>
      exact Ancestor.step ih edge

/-- Each direct parent edge strictly increases rank in the forward direction. -/
theorem parent_edge_rank_lt (h : GitHistory) {p c : Oid}
    (edge : ParentEdge h p c) : h.rank p < h.rank c := by
  rcases edge with ⟨commit, hcommit, hparent⟩
  exact h.parentRank c commit hcommit p hparent

/-- An ancestor can never have a greater rank than its descendant. -/
theorem ancestor_rank_le (h : GitHistory) {a d : Oid}
    (ha : Ancestor h a d) : h.rank a ≤ h.rank d := by
  induction ha with
  | refl => exact Nat.le_refl _
  | step hprev edge ih =>
      exact Nat.le_trans ih (Nat.le_of_lt (parent_edge_rank_lt h edge))

/-- A non-reflexive ancestor path strictly increases rank. This rules out a
    cycle in the represented ancestry relation, not just in direct parent edges. -/
theorem ancestor_rank_lt_of_ne (h : GitHistory) {a d : Oid}
    (ha : Ancestor h a d) (hne : a ≠ d) : h.rank a < h.rank d := by
  cases ha with
  | refl =>
      exact False.elim (hne rfl)
  | step hprev edge =>
      exact Nat.lt_of_le_of_lt (ancestor_rank_le h hprev)
        (parent_edge_rank_lt h edge)

/-- The modeled ancestry relation is antisymmetric: a finite ranked Git
    history cannot contain a nontrivial cycle. -/
theorem ancestry_antisymmetric (h : GitHistory) {a b : Oid}
    (hab : Ancestor h a b) (hba : Ancestor h b a) : a = b := by
  by_cases hne : a = b
  · exact hne
  · have hlt₁ : h.rank a < h.rank b := ancestor_rank_lt_of_ne h hab hne
    have hlt₂ : h.rank b < h.rank a :=
      ancestor_rank_lt_of_ne h hba (Ne.symm hne)
    exact (Nat.lt_irrefl (h.rank a) (Nat.lt_trans hlt₁ hlt₂)).elim

/-- A commit is in the base-to-head range iff base is its ancestor and it
    is itself an ancestor of head. -/
def InRange (h : GitHistory) (base head commit : Oid) : Prop :=
  Ancestor h base commit ∧ Ancestor h commit head

end GitHistory

/-! ## 1b. Local-to-global tree simulation obligation -/

/-- A tree-level refinement witness pairs the Git tree at each commit with
    the materialized tree represented by the translated Pijul state.

    `edgePreserves` is a proof obligation for the concrete translator: it must
    establish that a valid translation step preserves the representation
    relation. It is NOT an axiom about the upstream Pijul CLI. -/
structure TreeRefinement (h : GitHistory) where
  gitTree : Oid → GitTree
  pijulTree : Oid → GitTree
  edgePreserves : ∀ parent child,
    GitHistory.ParentEdge h parent child →
    TreeEq (gitTree parent) (pijulTree parent) →
    TreeEq (gitTree child) (pijulTree child)

/-- If the base commit is represented correctly and every translated parent
    edge preserves tree representation, every descendant on that history path
    is represented correctly. This composes local simulation obligations into
    a path-level theorem; it does not itself establish `edgePreserves`. -/
theorem tree_refinement_along_ancestry (h : GitHistory)
    (r : TreeRefinement h) {base head : Oid}
    (reach : GitHistory.Ancestor h base head)
    (baseCorrect : TreeEq (r.gitTree base) (r.pijulTree base)) :
    TreeEq (r.gitTree head) (r.pijulTree head) := by
  induction reach with
  | refl => exact baseCorrect
  | step hprev edge ih =>
      exact r.edgePreserves _ _ edge ih

/-! ## 2. Abstract change model and trust boundary -/

abbrev ChangeId := Nat

structure Patch where
  id           : ChangeId
  dependencies : List ChangeId
  deriving DecidableEq, Repr

/-- An abstract Pijul model. The concrete interpretation of patches, contexts,
    graph edges, commutation, conflicts and patch hashes must be defined from
    the selected upstream Pijul semantics before this interface can discharge
    end-to-end correctness.

    Valid is intentionally a semantic predicate. The bridge must not define
    it as "the CLI returned success". -/
structure PatchModel where
  State : Type
  stateEq : State → State → Prop
  PatchType : Type
  patchId : PatchType → ChangeId
  dependencies : PatchType → List ChangeId
  Valid : State → PatchType → Prop
  Apply : State → PatchType → Option State
  Conflict : State → PatchType → PatchType → Prop

/-! ## 3. P0 equivalence criteria: content and conflicts

P0 is restricted to synthetic repositories. Its candidate acceptance criterion
is content equivalence plus equivalence of conflict observations for explicitly
paired merge/apply scenarios. A Git commit does not intrinsically carry a
conflict predicate: gitConflict must mean the result of a specified merge
attempt in the test harness, and pijulConflict the result of the corresponding
specified Pijul operation. Those operations and their pairing must be defined
before this criterion can support a correctness claim.

Git commit topology is a separate objective and must not be smuggled into
P0's definition. These predicates define a target contract; they do not assert
that the current importer/exporter satisfies it. -/

structure P0Observation where
  imported : Oid → Prop
  gitTree : Oid → GitTree
  pijulTree : Oid → GitTree
  /-- Conflict result of a specified Git merge scenario, not an intrinsic
      property of a commit. -/
  gitConflict : Oid → Oid → Prop
  /-- Conflict result of the corresponding specified Pijul operation. -/
  pijulConflict : Oid → Oid → Prop

/-- P0 preserves every imported commit's materialized content and preserves
    the observed conflict result for each pair of imported tips. The test
    harness must define which operation each pair denotes. -/
def P0Equivalent (o : P0Observation) : Prop :=
  (∀ c, o.imported c → TreeEq (o.gitTree c) (o.pijulTree c)) ∧
  (∀ a b, o.imported a → o.imported b →
    (o.gitConflict a b ↔ o.pijulConflict a b))

/-- Exact parent-edge preservation is a distinct topology objective, not part
    of P0Equivalent. Its concrete target representation remains to be chosen. -/
def GitTopologyEquivalent (h : GitHistory)
    (translatedParent : Oid → Oid → Prop) : Prop :=
  ∀ p c, GitHistory.ParentEdge h p c ↔ translatedParent p c

/-! ## 4. Ledger contract -/

structure Ledger where
  entries : List (Oid × ChangeId)

/-- The ledger may not assign two distinct change identities to one Git OID.
    The reverse direction is not assumed injective: the design must decide
    whether shared ancestors reuse change identity. -/
def Ledger.Functional (ledger : Ledger) : Prop :=
  ∀ oid c₁ c₂, (oid, c₁) ∈ ledger.entries →
    (oid, c₂) ∈ ledger.entries → c₁ = c₂

theorem Ledger.recorded_change_unique (ledger : Ledger)
    (h : ledger.Functional) (oid : Oid) (c₁ c₂ : ChangeId)
    (h₁ : (oid, c₁) ∈ ledger.entries)
    (h₂ : (oid, c₂) ∈ ledger.entries) : c₁ = c₂ :=
  h oid c₁ c₂ h₁ h₂

/-- Append a mapping only when the exact pair is not already present. This
    small operation models duplicate suppression, not conflict resolution for
    a different change already associated with the same OID. -/
def Ledger.recordIfAbsent (ledger : Ledger) (oid : Oid) (change : ChangeId) : Ledger :=
  { entries :=
      if (oid, change) ∈ ledger.entries then
        ledger.entries
      else
        ledger.entries ++ [(oid, change)] }

/-- Recording the same mapping twice is idempotent at the ledger-data level.
    This theorem does not establish that an external import itself is
    idempotent, nor that a conflicting mapping for the same OID is accepted. -/
theorem Ledger.recordIfAbsent_idempotent (ledger : Ledger)
    (oid : Oid) (change : ChangeId) :
    (ledger.recordIfAbsent oid change).recordIfAbsent oid change =
      ledger.recordIfAbsent oid change := by
  by_cases h : (oid, change) ∈ ledger.entries
  · simp [Ledger.recordIfAbsent, h]
  · simp [Ledger.recordIfAbsent, h]


/-- A checked ledger write rejects an OID that is already mapped to a
    different change. Exact duplicate writes remain accepted and idempotent. -/
def Ledger.recordChecked (ledger : Ledger) (oid : Oid) (change : ChangeId) :
    Option Ledger :=
  if ledger.entries.any (fun entry => entry.1 = oid ∧ entry.2 ≠ change) then
    none
  else
    some (ledger.recordIfAbsent oid change)

/-- Conflicting OID→change associations fail closed instead of adding a
    second, contradictory mapping. -/
theorem Ledger.recordChecked_rejects_conflict (ledger : Ledger)
    (oid existing change : ChangeId)
    (hEntry : (oid, existing) ∈ ledger.entries)
    (hDifferent : existing ≠ change) :
    ledger.recordChecked oid change = none := by
  have hAny :
      ledger.entries.any (fun entry => entry.1 = oid ∧ entry.2 ≠ change) = true := by
    rw [List.any_eq_true]
    refine ⟨(oid, existing), hEntry, ?_⟩
    simp only [decide_eq_true]
    exact ⟨rfl, hDifferent⟩
  rw [Ledger.recordChecked, if_pos hAny]

/-- If no conflicting association exists, the checked write returns the
    duplicate-suppressing update. This does not claim that the external
    importer generated the correct change in the first place. -/
theorem Ledger.recordChecked_accepts_consistent (ledger : Ledger)
    (oid : Oid) (change : ChangeId)
    (hNoConflict :
      ¬ ∃ previous, (oid, previous) ∈ ledger.entries ∧ previous ≠ change) :
    ledger.recordChecked oid change =
      some (ledger.recordIfAbsent oid change) := by
  have hAny :
      ledger.entries.any (fun entry => entry.1 = oid ∧ entry.2 ≠ change) = false := by
    cases hh :
        ledger.entries.any (fun entry => entry.1 = oid ∧ entry.2 ≠ change) with
    | false => rfl
    | true =>
        rw [List.any_eq_true] at hh
        obtain ⟨pair, hmem, hp⟩ := hh
        have hp' : pair.1 = oid ∧ pair.2 ≠ change := of_decide_eq_true hp
        have hmem' : (oid, pair.2) ∈ ledger.entries := by
          rw [← hp'.1]
          exact hmem
        exact (hNoConflict ⟨pair.2, hmem', hp'.2⟩).elim
  rw [Ledger.recordChecked, if_neg (by simp [hAny])]

/-- A mapping observation collected from one import operation. -/
structure MappingObservation where
  gitOid : Oid
  change : ChangeId
  before : List ChangeId
  after : List ChangeId

/-- Singleton delta condition for deriving a mapping from before/after lists.
    Callers must canonicalize and deduplicate these lists before using this
    condition as a set difference. -/
def IsSingletonDelta (obs : MappingObservation) : Prop :=
  obs.change ∈ obs.after ∧
  obs.change ∉ obs.before ∧
  ∀ c, c ∈ obs.after → c ∉ obs.before → c = obs.change

/-- If a before/after observation satisfies the singleton-delta contract,
    the newly introduced change is the unique member of the set difference.
    The caller is responsible for supplying canonicalized, duplicate-free
    lists; this theorem does not identify a diff with upstream Pijul semantics. -/
theorem singleton_delta_unique (obs : MappingObservation)
    (hdelta : IsSingletonDelta obs) (candidate : ChangeId)
    (hafter : candidate ∈ obs.after)
    (hnew : candidate ∉ obs.before) : candidate = obs.change :=
  hdelta.2.2 candidate hafter hnew

/-- Two observations with the same canonical before/after set difference
    cannot report different changes if both satisfy the singleton-delta
    contract. The OID is not needed for this purely set-theoretic lemma. -/
theorem singleton_delta_same_difference
    (obs₁ obs₂ : MappingObservation)
    (hBefore : obs₁.before = obs₂.before)
    (hAfter : obs₁.after = obs₂.after)
    (h₁ : IsSingletonDelta obs₁) (h₂ : IsSingletonDelta obs₂) :
    obs₁.change = obs₂.change := by
  have h₁in : obs₁.change ∈ obs₂.after := by
    rw [← hAfter]
    exact h₁.1
  have h₁new : obs₁.change ∉ obs₂.before := by
    rw [← hBefore]
    exact h₁.2.1
  have h₂in : obs₂.change ∈ obs₁.after := by
    rw [hAfter]
    exact h₂.1
  have h₂new : obs₂.change ∉ obs₁.before := by
    rw [hBefore]
    exact h₂.2.1
  have h₁eq : obs₁.change = obs₂.change :=
    h₂.2.2 obs₁.change h₁in h₁new
  exact h₁eq

/-! ## 5. Import protocol and fail-closed publication -/

inductive OperationStatus
  | pending
  | imported
  | rejected
  deriving DecidableEq, Repr

structure ImportRecord where
  gitOid : Oid
  status : OperationStatus
  mappedChange : Option ChangeId
  deriving DecidableEq, Repr

/-- An imported record must carry a mapping. -/
def RecordWellFormed (r : ImportRecord) : Prop :=
  match r.status, r.mappedChange with
  | .imported, some _ => True
  | .imported, none => False
  | .pending, _ => True
  | .rejected, _ => True

theorem imported_record_has_mapping (r : ImportRecord)
    (h : RecordWellFormed r) (hs : r.status = .imported) :
    ∃ change, r.mappedChange = some change := by
  cases hm : r.mappedChange with
  | none =>
      simp [RecordWellFormed, hs, hm] at h
  | some change =>
      refine ⟨change, ?_⟩
      rw [← hm]

/-! ## 6. Required correctness obligations (not yet proved)

The following targets must become formal predicates and theorems after the
concrete semantics and refinement relation are defined.

* Ancestry/dependency preservation: every dependency emitted by translation
  is justified by source history or by a separately specified Pijul dependency.
* Patch fidelity: translated patches satisfy the selected Pijul semantics;
  equality of materialized file trees alone is insufficient.
* Conflict preservation: incompatible concurrent source changes are not
  silently dropped or advertised as a conflict-free successful translation.
* Mapping soundness: each published Git OID -> change mapping corresponds to
  exactly the source commit under the chosen translation relation.
* Idempotence: rerunning an import leaves the abstract result unchanged.
* Safe publication: output refs are published only after all required
  invariants hold; external command failure cannot be treated as success.

No end-to-end correctness theorem is claimed in this file.
-/

end PijulBridge

namespace PijulBridge

/-! ## 7. Platform-level Git ↔ Pijul contract (definition layer)

These definitions refine the synthetic P0 criterion into a platform contract.
They are intentionally abstract: they define what a correct bridge must
preserve, not what the current Rust importer or upstream Pijul CLI already
satisfies. In particular, no conflict-resolution behavior is assumed of Pijul.

The path type `Path` is a canonicalized path identity. Observable entries use
bytes and file kind rather than comparing Git blob IDs with Pijul internals.
Gitlinks/submodules are represented explicitly so a concrete implementation
must either support them or state a narrower supported-tree precondition.
-/

/-- The observable kind and payload of a path in a materialized repository.
    Symlink payload is the target's byte sequence; gitlink payload is the
    referenced Git commit identity. -/
inductive ObservableEntry where
  | file (bytes : List UInt8) (executable : Bool)
  | symlink (targetBytes : List UInt8)
  | gitlink (target : Oid)
  deriving DecidableEq, Repr

/-- A normalized, materialized tree. Directory entries are implicit in paths;
    a concrete adapter must define path normalization and platform-specific
    metadata policy. -/
abbrev ObservableTree := Path → Option ObservableEntry

def ObservableTreeEq (a b : ObservableTree) : Prop :=
  ∀ path, a path = b path

theorem observableTreeEq_refl (tree : ObservableTree) :
    ObservableTreeEq tree tree := by
  intro path
  rfl

theorem observableTreeEq_symm {a b : ObservableTree}
    (h : ObservableTreeEq a b) : ObservableTreeEq b a := by
  intro path
  exact (h path).symm

theorem observableTreeEq_trans {a b c : ObservableTree}
    (hab : ObservableTreeEq a b) (hbc : ObservableTreeEq b c) :
    ObservableTreeEq a c := by
  intro path
  exact Eq.trans (hab path) (hbc path)

/-- Native conflict classes are labels for operations that the selected Pijul
    semantics actually reports as conflicting. They are not inferred from Git
    conflict markers and are not a claim about business-level correctness. -/
inductive NativeConflictOperation where
  | applyChange
  | commuteChanges
  | mergeChanges
  deriving DecidableEq, Repr

structure NativeConflict where
  leftChange : ChangeId
  rightChange : ChangeId
  operation : NativeConflictOperation
  deriving DecidableEq, Repr

/-- A semantic view of Git. The observation function is deliberately separate
    from the earlier hash-based GitTree model. -/
structure GitView where
  history : GitHistory
  tip : Oid
  treeAt : Oid → ObservableTree
  ref : Nat → Option Oid

/-- A semantic view of Pijul at one selected state. Dependencies are oriented
    as (change, prerequisite). The native conflict list is the result of a
    specified Pijul operation, not a generic property of a repository. -/
structure PijulView where
  changes : List ChangeId
  dependsOn : ChangeId → ChangeId → Prop
  tree : ObservableTree
  nativeConflicts : List NativeConflict

/-- Cross-system provenance is a relation, not a bijection. This deliberately
    permits distinct native histories and avoids assuming that a Git commit
    and a Pijul change have identical identity or cardinality. -/
structure BridgeLedger where
  links : Oid → ChangeId → Prop
  importedCommit : Oid → Prop
  requiresChangeMapping : Oid → Prop
  bridgeCreatedChange : ChangeId → Prop

structure PlatformState where
  git : GitView
  pijul : PijulView
  ledger : BridgeLedger

/-- Every recorded provenance link refers to an existing Git commit and a
    change present in the selected Pijul view. -/
def MappingSound (s : PlatformState) : Prop :=
  ∀ oid change, s.ledger.links oid change →
    (∃ commit, s.git.history.commit oid = some commit) ∧
    change ∈ s.pijul.changes

/-- Every imported commit that the translation contract says requires a
    change mapping has at least one mapping. Empty/metadata-only commits may
    set requiresChangeMapping to false; their handling must still be specified
    by the concrete adapter if their metadata/topology matters. -/
def MappingCoverage (s : PlatformState) : Prop :=
  ∀ oid, s.ledger.importedCommit oid →
    s.ledger.requiresChangeMapping oid →
    ∃ change, s.ledger.links oid change

/-- Every change created by the bridge has traceable Git provenance. Native
    Pijul changes are not required to link to Git commits. -/
def BridgeChangeTraceability (s : PlatformState) : Prop :=
  ∀ change, s.ledger.bridgeCreatedChange change →
    ∃ oid, s.ledger.links oid change

/-- Dependency closure for the selected Pijul state: a selected change cannot
    refer to a prerequisite that is absent from that state. -/
def DependencyClosed (p : PijulView) : Prop :=
  ∀ change prerequisite, change ∈ p.changes →
    p.dependsOn change prerequisite → prerequisite ∈ p.changes

/-- Human arbitration is required exactly when the specified Pijul operation
    reports at least one native conflict. There is no external semantic
    conflict detector in this contract. -/
def ArbitrationRequired (p : PijulView) : Prop :=
  p.nativeConflicts ≠ []

/-- A conflict scenario pairs a concrete Git merge attempt with a concrete
    Pijul operation over the corresponding concurrent work. It records the
    outcomes separately: there is no axiom that their conflict booleans agree. -/
structure ConflictScenario where
  base : Oid
  leftTip : Oid
  rightTip : Oid
  gitMergeReportedConflict : Bool
  pijulConflicts : List NativeConflict
  pijulMaterializedTree : ObservableTree

/-- A Git merge conflict has been handled without arbitration only when the
    paired Pijul operation reports no native conflict. This is a classification
    predicate, not a theorem that such a Pijul operation always exists. -/
def GitConflictHasNoPijulArbitrationSignal (scenario : ConflictScenario) : Prop :=
  scenario.gitMergeReportedConflict = true ∧ scenario.pijulConflicts = []

/-- The arbitration signal for a scenario is determined solely by native
    Pijul conflicts. Git's conflict result alone never triggers arbitration. -/
def ScenarioRequiresArbitration (scenario : ConflictScenario) : Prop :=
  scenario.pijulConflicts ≠ []

/-- The state is coherent at a quiescent synchronization point when the
    materialized trees agree (under the normalized observation relation) and
    cross-system provenance, dependency closure, and bridge-change traceability
    hold. An unresolved native conflict makes this state non-quiescent and is
    handled by the separate pending-arbitration predicate below. -/
def CoherentCheckpoint (s : PlatformState) : Prop :=
  ObservableTreeEq (s.git.treeAt s.git.tip) s.pijul.tree ∧
  MappingSound s ∧
  MappingCoverage s ∧
  BridgeChangeTraceability s ∧
  DependencyClosed s.pijul ∧
  s.pijul.nativeConflicts = []

/-- A conflict-bearing state is a valid pending-arbitration state when its
    provenance and dependency invariants remain valid. Its materialized trees
    are not claimed equivalent until resolution and a later checkpoint. -/
def PendingArbitration (s : PlatformState) : Prop :=
  s.pijul.nativeConflicts ≠ [] ∧
  MappingSound s ∧
  MappingCoverage s ∧
  BridgeChangeTraceability s ∧
  DependencyClosed s.pijul

/-- Platform coherence permits either a quiescent synchronized checkpoint or
    an explicitly pending native Pijul arbitration. It does not classify
    silent semantic incompatibilities as conflicts; those are out of scope. -/
def PlatformCoherent (s : PlatformState) : Prop :=
  CoherentCheckpoint s ∨ PendingArbitration s

/-! ## 8. Required simulation laws (definitions of proof targets)

The following predicates make the remaining proof obligations precise.
They do not yet quantify over concrete Rust executions; a refinement theorem
must connect each concrete operation to these abstract transitions.
-/

/-- A successful synchronization must end at a coherent checkpoint. A
    conflict-bearing synchronization must instead expose a pending native
    Pijul arbitration. -/
def SynchronizationCorrect (sync : PlatformState → PlatformState)
    (before : PlatformState) : Prop :=
  PlatformCoherent (sync before)

/-- Idempotence is stated over abstract platform observations: running the same
    synchronization twice must not change the resulting selected trees,
    provenance relation, or conflict observation. This avoids demanding
    identical internal allocation or native history representation. -/
def SamePlatformObservation (a b : PlatformState) : Prop :=
  ObservableTreeEq (a.git.treeAt a.git.tip) (b.git.treeAt b.git.tip) ∧
  ObservableTreeEq a.pijul.tree b.pijul.tree ∧
  (∀ oid change, a.ledger.links oid change ↔ b.ledger.links oid change) ∧
  a.pijul.nativeConflicts = b.pijul.nativeConflicts

def SynchronizationIdempotent (sync : PlatformState → PlatformState)
    (s : PlatformState) : Prop :=
  SamePlatformObservation (sync (sync s)) (sync s)

/-- A Git-only or Pijul-only workflow may advance its native repository without
    requiring the other system's native history to be rewritten. The concrete
    implementation must define how divergence is represented until the next
    synchronization; autonomy does not mean the two views remain synchronized
    after every unilateral edit. -/
structure AutonomousWorkflows where
  gitStep : GitView → GitView → Prop
  pijulStep : PijulView → PijulView → Prop

/-- One native Git step leaves the native Pijul view untouched. The ledger may
    record the new Git-side state, but this predicate does not force immediate
    synchronization or tree equality. -/
def GitAutonomousStep (wf : AutonomousWorkflows)
    (before after : PlatformState) : Prop :=
  wf.gitStep before.git after.git ∧ after.pijul = before.pijul

/-- One native Pijul step leaves the native Git view untouched. Native Pijul
    changes may be added without manufacturing Git commits; synchronization
    is a separate operation. -/
def PijulAutonomousStep (wf : AutonomousWorkflows)
    (before after : PlatformState) : Prop :=
  wf.pijulStep before.pijul after.pijul ∧ after.git = before.git

/-- Git bootstrap is based on the observable Git history, refs and trees only.
    This contract intentionally has no field for metadata unavailable from Git.
    A concrete bootstrap must either satisfy the stated mapping obligations or
    reject/report cases for which its supported input contract is insufficient. -/
def BootstrapUsesGitObservableData (source : GitView) (target : PlatformState) : Prop :=
  target.git.history = source.history ∧
  target.git.tip = source.tip ∧
  target.git.ref = source.ref

/-! ## 9. Theorems still required (not asserted here)

The eventual bridge theorem should prove, for every supported Git history and
every allowed sequence of Git/Pijul work:
1. bootstrap only uses observable Git data and either succeeds soundly or
   fails explicitly;
2. each successful translation preserves normalized tree observations;
3. provenance is sound and complete for commits requiring a change mapping;
4. Pijul dependency closure and native conflict observations are preserved;
5. a Git-reported merge conflict is resolved without arbitration only when the
   paired Pijul operation is conflict-free; otherwise native Pijul conflicts
   are surfaced for arbitration;
6. synchronization is idempotent at the observation level and publishes no
   invalid partial state;
7. either ecosystem can continue independently, with divergence allowed
   between synchronization points.

None of these end-to-end properties follows from the definitions alone.
The actual Pijul operation semantics and the Rust implementation must be
connected to these predicates; tests and the CLI exit status are not proofs.
-/

end PijulBridge
