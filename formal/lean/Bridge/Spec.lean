/-
Copyright (c) 2026 Martin-PB. All rights reserved.
See the repository root for the applicable license.

Git ↔ Pijul bridge — first formal specification layer.

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
  | @step _ parent child hbp edge ih =>
      exact Ancestor.step ih edge

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
  Patch : Type
  patchId : Patch → ChangeId
  dependencies : Patch → List ChangeId
  Valid : State → Patch → Prop
  Apply : State → Patch → Option State
  Conflict : State → Patch → Patch → Prop

/-! ## 3. Ledger contract -/

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

/-! ## 4. Import protocol and fail-closed publication -/

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

/-! ## 5. Required correctness obligations (not yet proved)

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

No end-to-end correctness theorem is claimed in this initial file.
-/

end PijulBridge
