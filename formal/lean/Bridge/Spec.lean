/*
Copyright (c) 2026 Martin-PB. All rights reserved.
See the repository root for the applicable license.

Git ↔ Pijul bridge — formal specification layers.

This file specifies contracts before choosing an import algorithm. It does NOT
claim to model the full Pijul patch calculus yet. The abstract Pijul interface
below is an explicit proof boundary, not a theorem about the upstream CLI. Do
not add axioms that simply assert the desired end-to-end correctness property.
*/

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
  | refl _ => exact hab
  | @step _ _ parent child hbp edge ih =>
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
  | refl x => exact Nat.le_refl _
  | @step ancestor parent child hprev edge ih =>
      exact Nat.le_trans ih (Nat.le_of_lt (parent_edge_rank_lt h edge))

/-- A commit is in the base-to-head range iff base is its ancestor and it
    is itself an ancestor of head. -/
def InRange (h : GitHistory) (base head commit : Oid) : Prop :=
  Ancestor h base commit ∧ Ancestor h commit head

end GitHistory

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

P0 is restricted to synthetic repositories. Its acceptance criterion is
content equivalence plus conflict-observation equivalence. Git commit topology
is a separate objective and must not be smuggled into P0's definition.
These predicates define the target contract; they do not assert that the
current importer/exporter satisfies it. -/

structure P0Observation where
  imported : Oid → Prop
  gitTree : Oid → GitTree
  pijulTree : Oid → GitTree
  gitConflict : Oid → Oid → Prop
  pijulConflict : Oid → Oid → Prop

/-- P0 preserves every imported commit's materialized content and preserves
    the conflict relation observed for imported commit pairs. -/
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
      exact ⟨change, hm⟩

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
