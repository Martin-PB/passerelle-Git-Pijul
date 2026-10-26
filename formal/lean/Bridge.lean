/-
Root module of the `Bridge` Lean library.

The lakefile declares `lean_lib Bridge` with source directory `formal/lean`,
so Lake resolves the library root as `formal/lean/Bridge.lean`. This file did
not exist (only `formal/lean/Bridge/Spec.lean` did), which made every
`lake build Bridge` fail with:

    error: no such file or directory (error code 2)
      file: ./././formal/lean/Bridge.lean

This root module simply imports the actual specification so that the whole
tree compiles transitively from the library root.
-/
import Bridge.Spec
