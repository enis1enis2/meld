# Source this profile snippet to make the bundled Meld Lang Packs discoverable.
# It exports no runtime installation and never downloads anything.
MELD_PACKS="/usr/share/meld-lang/packs${MELD_PACKS:+:$MELD_PACKS}"
export MELD_PACKS
