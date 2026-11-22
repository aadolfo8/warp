#!/usr/bin/env python3
"""Vendors VS Code's Material Icon Theme into Warp's bundled assets.

Usage:
    npm pack material-icon-theme && tar -xzf material-icon-theme-*.tgz
    python script/update_material_icons.py path/to/extracted/package

Copies every icon SVG into app/assets/bundled/svg/material/ and writes a
compact manifest (icon names resolved to SVG file names) to
app/src/code/material_icons/manifest.json, which app/src/code/material_icons.rs
embeds. The theme is MIT licensed; its LICENSE is copied next to the icons.
"""

import json
import os
import shutil
import sys

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
ICONS_OUT = os.path.join(REPO_ROOT, "app", "assets", "bundled", "svg", "material")
MANIFEST_OUT = os.path.join(REPO_ROOT, "app", "src", "code", "material_icons", "manifest.json")


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    package = sys.argv[1]
    with open(os.path.join(package, "dist", "material-icons.json"), encoding="utf-8") as f:
        theme = json.load(f)
    with open(os.path.join(package, "package.json"), encoding="utf-8") as f:
        version = json.load(f)["version"]

    definitions = theme["iconDefinitions"]

    def svg_name(icon_id: str) -> str:
        """The bundled SVG file name (without .svg) for a theme icon id."""
        return os.path.splitext(os.path.basename(definitions[icon_id]["iconPath"]))[0]

    def resolve(mapping: dict) -> dict:
        return {key.lower(): svg_name(icon_id) for key, icon_id in sorted(mapping.items())}

    manifest = {
        "version": version,
        "file": svg_name(theme["file"]),
        "folder": svg_name(theme["folder"]),
        "folderExpanded": svg_name(theme["folderExpanded"]),
        "rootFolder": svg_name(theme["rootFolder"]),
        "rootFolderExpanded": svg_name(theme["rootFolderExpanded"]),
        "fileNames": resolve(theme["fileNames"]),
        "fileExtensions": resolve(theme["fileExtensions"]),
        # Expanded folder icons are always the closed icon id plus "-open".
        "folderNames": {
            name.lower(): [svg_name(icon_id), svg_name(icon_id + "-open")]
            for name, icon_id in sorted(theme["folderNames"].items())
        },
    }

    if os.path.isdir(ICONS_OUT):
        shutil.rmtree(ICONS_OUT)
    os.makedirs(ICONS_OUT)
    used = {svg_name(icon_id) for icon_id in definitions}
    for name in sorted(used):
        shutil.copyfile(os.path.join(package, "icons", name + ".svg"), os.path.join(ICONS_OUT, name + ".svg"))
    shutil.copyfile(os.path.join(package, "LICENSE"), os.path.join(ICONS_OUT, "LICENSE"))

    os.makedirs(os.path.dirname(MANIFEST_OUT), exist_ok=True)
    with open(MANIFEST_OUT, "w", encoding="utf-8", newline="\n") as f:
        json.dump(manifest, f, separators=(",", ":"), sort_keys=True)
        f.write("\n")

    print(f"Material Icon Theme {version}: {len(used)} icons, "
          f"{len(manifest['fileNames'])} file names, {len(manifest['fileExtensions'])} extensions, "
          f"{len(manifest['folderNames'])} folder names")


if __name__ == "__main__":
    main()
