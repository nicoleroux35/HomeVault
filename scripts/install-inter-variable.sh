#!/bin/bash

# Download and install Inter Variable font
# Optimizes HomeVault fonts: reduces 40+ OTF files to 2 WOFF2 files

echo "📦 Installation Inter Variable pour HomeVault"
echo "=============================================="
echo ""

FONTS_DIR="frontend/public/fonts/inter"
TEMP_DIR="/tmp/inter-download"

# Create temporary directory
mkdir -p "$TEMP_DIR"

echo "⬇️  Téléchargement d'Inter Variable..."
cd "$TEMP_DIR"

# Download Inter from GitHub
wget -q https://github.com/rsms/inter/releases/download/v4.0/Inter-4.0.zip

if [ ! -f "Inter-4.0.zip" ]; then
  echo "❌ Erreur de téléchargement"
  exit 1
fi

echo "📦 Extraction..."
unzip -q Inter-4.0.zip

# Verify files exist
if [ ! -d "Inter Desktop" ]; then
  echo "❌ Structure de fichiers inattendue"
  exit 1
fi

echo "📂 Copie des fichiers Variable..."
cd "../$(pwd | xargs basename)"

# Copier les fichiers Variable WOFF2
if [ -f "$TEMP_DIR/Inter Desktop/Inter-Variable.woff2" ]; then
  cp "$TEMP_DIR/Inter Desktop/Inter-Variable.woff2" "$FONTS_DIR/"
  echo "  ✅ Inter-Variable.woff2 copié"
fi

if [ -f "$TEMP_DIR/Inter Desktop/Inter-Variable-Italic.woff2" ]; then
  cp "$TEMP_DIR/Inter Desktop/Inter-Variable-Italic.woff2" "$FONTS_DIR/"
  echo "  ✅ Inter-Variable-Italic.woff2 copié"
fi

# Nettoyer
rm -rf "$TEMP_DIR"

echo ""
echo "📊 Comparaison:"
echo "  Avant: 40+ fichiers OTF (~3-4 MB)"
echo "  Après: 2 fichiers WOFF2 (~200-300 KB)"
echo "  Gain: ~90% de réduction 🎉"
echo ""
echo "✅ Installation terminée!"
echo ""
echo "📝 Prochaines étapes:"
echo "  1. Importer fonts.css dans main.ts:"
echo "     import '\$lib/styles/fonts.css';"
echo "  2. Ajouter preload dans index.html (optionnel)"
echo "  3. (Optionnel) Supprimer les anciens OTF:"
echo "     rm frontend/public/fonts/inter/*.otf"
echo ""
