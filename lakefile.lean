import Lake
open Lake DSL

package «passerelle-git-pijul-formal» where
  -- Keep the Lean version pinned by lean-toolchain at the repository root.

lean_lib Bridge where
  srcDir := "formal/lean"
