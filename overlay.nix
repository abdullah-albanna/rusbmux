final: prev: {
  rusbmux = import ./default.nix {
    inherit (prev) lib rustPlatform;
  };
}
