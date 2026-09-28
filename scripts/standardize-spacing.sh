#!/bin/bash

# Automatic spacing standardization script
# HomeVault - Migration to CSS variables

echo "🔧 Standardisation des espacements HomeVault"
echo "=============================================="

# Target directory
TARGET_DIR="${1:-./frontend/src}"

if [ ! -d "$TARGET_DIR" ]; then
  echo "❌ Erreur: Répertoire $TARGET_DIR introuvable"
  exit 1
fi

echo "📁 Répertoire cible: $TARGET_DIR"
echo ""

# Compteur de modifications
MODIFIED=0

# Fonction de remplacement avec sauvegarde
replace_spacing() {
  local pattern="$1"
  local replacement="$2"
  local description="$3"

  echo "🔍 Recherche: $description"

  # Trouver et remplacer dans tous les fichiers .svelte et .css
  find "$TARGET_DIR" -type f \( -name "*.svelte" -o -name "*.css" \) -exec grep -l "$pattern" {} \; 2>/dev/null | while read file; do
    # Create a backup
    cp "$file" "$file.bak"

    # Effectuer le remplacement
    sed -i "s/$pattern/$replacement/g" "$file"

    # Check if the file changed
    if ! cmp -s "$file" "$file.bak"; then
      echo "  ✅ Modifié: $file"
      MODIFIED=$((MODIFIED + 1))
      rm "$file.bak"
    else
      rm "$file.bak"
    fi
  done
}

echo "📝 Application des remplacements..."
echo ""

# Remplacements padding/margin/gap les plus courants
# Note: Order matters to avoid double replacements

# Gap
replace_spacing "gap: 24px" "gap: var(--spacing-6)" "gap: 24px → var(--spacing-6)"
replace_spacing "gap: 20px" "gap: var(--spacing-5)" "gap: 20px → var(--spacing-5)"
replace_spacing "gap: 16px" "gap: var(--spacing-4)" "gap: 16px → var(--spacing-4)"
replace_spacing "gap: 12px" "gap: var(--spacing-3)" "gap: 12px → var(--spacing-3)"
replace_spacing "gap: 10px" "gap: var(--spacing-2-5)" "gap: 10px → var(--spacing-2-5)"
replace_spacing "gap: 8px" "gap: var(--spacing-2)" "gap: 8px → var(--spacing-2)"
replace_spacing "gap: 6px" "gap: var(--spacing-1-5)" "gap: 6px → var(--spacing-1-5)"
replace_spacing "gap: 4px" "gap: var(--spacing-1)" "gap: 4px → var(--spacing-1)"
replace_spacing "gap: 3px" "gap: var(--spacing-0-75)" "gap: 3px → var(--spacing-0-75)"
replace_spacing "gap: 2px" "gap: var(--spacing-0-5)" "gap: 2px → var(--spacing-0-5)"

# Padding simple (une valeur)
replace_spacing "padding: 48px" "padding: var(--spacing-12)" "padding: 48px"
replace_spacing "padding: 40px" "padding: var(--spacing-10)" "padding: 40px"
replace_spacing "padding: 32px" "padding: var(--spacing-8)" "padding: 32px"
replace_spacing "padding: 24px" "padding: var(--spacing-6)" "padding: 24px"
replace_spacing "padding: 20px" "padding: var(--spacing-5)" "padding: 20px"
replace_spacing "padding: 16px" "padding: var(--spacing-4)" "padding: 16px"
replace_spacing "padding: 12px" "padding: var(--spacing-3)" "padding: 12px"
replace_spacing "padding: 10px" "padding: var(--spacing-2-5)" "padding: 10px"
replace_spacing "padding: 8px" "padding: var(--spacing-2)" "padding: 8px"
replace_spacing "padding: 6px" "padding: var(--spacing-1-5)" "padding: 6px"
replace_spacing "padding: 4px" "padding: var(--spacing-1)" "padding: 4px"
replace_spacing "padding: 3px" "padding: var(--spacing-0-75)" "padding: 3px"
replace_spacing "padding: 2px" "padding: var(--spacing-0-5)" "padding: 2px"

# Margin simple (une valeur)
replace_spacing "margin: 24px" "margin: var(--spacing-6)" "margin: 24px"
replace_spacing "margin: 16px" "margin: var(--spacing-4)" "margin: 16px"
replace_spacing "margin: 12px" "margin: var(--spacing-3)" "margin: 12px"
replace_spacing "margin: 8px" "margin: var(--spacing-2)" "margin: 8px"
replace_spacing "margin: 4px" "margin: var(--spacing-1)" "margin: 4px"

# Padding/Margin avec plusieurs valeurs (patterns courants)
replace_spacing "padding: 0 16px" "padding: 0 var(--spacing-4)" "padding: 0 16px"
replace_spacing "padding: 0 12px" "padding: 0 var(--spacing-3)" "padding: 0 12px"
replace_spacing "padding: 0 8px" "padding: 0 var(--spacing-2)" "padding: 0 8px"
replace_spacing "margin: 0 8px" "margin: 0 var(--spacing-2)" "margin: 0 8px"
replace_spacing "margin: 0 4px" "margin: 0 var(--spacing-1)" "margin: 0 4px"

# Top/Right/Bottom/Left individuels
replace_spacing "padding-top: 16px" "padding-top: var(--spacing-4)" "padding-top: 16px"
replace_spacing "padding-bottom: 16px" "padding-bottom: var(--spacing-4)" "padding-bottom: 16px"
replace_spacing "padding-left: 16px" "padding-left: var(--spacing-4)" "padding-left: 16px"
replace_spacing "padding-right: 16px" "padding-right: var(--spacing-4)" "padding-right: 16px"

replace_spacing "margin-top: 16px" "margin-top: var(--spacing-4)" "margin-top: 16px"
replace_spacing "margin-bottom: 16px" "margin-bottom: var(--spacing-4)" "margin-bottom: 16px"
replace_spacing "margin-left: 8px" "margin-left: var(--spacing-2)" "margin-left: 8px"
replace_spacing "margin-right: 8px" "margin-right: var(--spacing-2)" "margin-right: 8px"

# Border-radius
replace_spacing "border-radius: 12px" "border-radius: var(--radius-2xl)" "border-radius: 12px"
replace_spacing "border-radius: 8px" "border-radius: var(--radius-lg)" "border-radius: 8px"
replace_spacing "border-radius: 6px" "border-radius: var(--radius-md)" "border-radius: 6px"
replace_spacing "border-radius: 4px" "border-radius: var(--radius-base)" "border-radius: 4px"

# Font sizes
replace_spacing "font-size: 14px" "font-size: var(--text-base)" "font-size: 14px"
replace_spacing "font-size: 13px" "font-size: var(--text-sm)" "font-size: 13px"
replace_spacing "font-size: 12px" "font-size: var(--text-xs)" "font-size: 12px (peut être var(--text-xs))"
replace_spacing "font-size: 11px" "font-size: var(--text-xs)" "font-size: 11px"

echo ""
echo "✨ Standardisation terminée!"
echo ""
echo "📊 Statistiques:"
echo "  - Fichiers modifiés: Vérifier les logs ci-dessus"
echo "  - Sauvegardes créées: *.bak (à supprimer après vérification)"
echo ""
echo "⚠️  IMPORTANT:"
echo "  1. Vérifiez les changements visuellement"
echo "  2. Testez l'application en dev"
echo "  3. Vérifiez le mode dark"
echo "  4. Si tout est OK, supprimez les fichiers .bak"
echo ""
echo "🗑️  Pour supprimer les sauvegardes:"
echo "  find $TARGET_DIR -name '*.bak' -delete"
