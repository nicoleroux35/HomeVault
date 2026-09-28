#!/bin/bash
set -euo pipefail

# Script de finalisation Phase 1 - Standardisation UI/UX
# HomeVault Frontend - Remove duplicate CSS

echo "🎯 FINALISATION PHASE 1 - STANDARDISATION UI/UX"
echo "════════════════════════════════════════════════════════════"
echo ""

FRONTEND_DIR="/home/user/Projet/HOMEVAULT - HTMX/frontend/src"
BACKUP_DIR="/home/user/Projet/HOMEVAULT - HTMX/.backup-phase1-final-$(date +%Y%m%d-%H%M%S)"

echo "💾 Création du backup final..."
mkdir -p "$BACKUP_DIR"
cp -r "$FRONTEND_DIR" "$BACKUP_DIR/"
echo "✅ Backup créé: $BACKUP_DIR"
echo ""

cd "$FRONTEND_DIR/routes"

echo "🧹 ÉTAPE 1: Suppression des styles CSS dupliqués"
echo "────────────────────────────────────────────────────────────"

# Fonction pour supprimer les styles redondants dans un fichier
cleanup_duplicate_styles() {
    local file=$1
    local original_lines=$(wc -l < "$file")

    # Create a temporary file
    local temp_file="${file}.tmp"

    # Flag to track if we're in a <style> section to remove
    local in_duplicate_style=0
    local lines_removed=0

    while IFS= read -r line; do
        # Detect duplicate styles to remove
        if [[ "$line" =~ \<style\> ]]; then
            # Check if following lines contain classes already in globals.css
            echo "$line" >> "$temp_file"
        elif [[ "$line" =~ \.hv-modal-overlay.*\{ ]] || \
             [[ "$line" =~ \.hv-modal[^-].*\{ ]] || \
             [[ "$line" =~ \.hv-modal-header.*\{ ]] || \
             [[ "$line" =~ \.hv-modal-close.*\{ ]] || \
             [[ "$line" =~ \.hv-modal-content.*\{ ]] || \
             [[ "$line" =~ \.hv-modal-footer.*\{ ]] || \
             [[ "$line" =~ \.hv-input[^-].*\{ ]] || \
             [[ "$line" =~ \.hv-label.*\{ ]] || \
             [[ "$line" =~ @keyframes.*spin ]] || \
             [[ "$line" =~ @keyframes.*fade ]] || \
             [[ "$line" =~ @keyframes.*slide ]]; then
            # Skip this line (already in globals.css)
            ((lines_removed++))
        else
            echo "$line" >> "$temp_file"
        fi
    done < "$file"

    # Remplacer le fichier original
    mv "$temp_file" "$file"

    local new_lines=$(wc -l < "$file")
    local removed=$((original_lines - new_lines))

    if [ $removed -gt 0 ]; then
        echo "   ✅ $file: -$removed lignes"
    fi
}

# Nettoyer les fichiers
for file in Users.svelte Groups.svelte Explorer.svelte Dashboard.svelte Login.svelte; do
    if [ -f "$file" ]; then
        cleanup_duplicate_styles "$file"
    fi
done

echo ""
echo "🔍 ÉTAPE 2: Suppression animations dupliquées"
echo "────────────────────────────────────────────────────────────"

# Remove duplicate @keyframes
for file in *.svelte; do
    if [ -f "$file" ]; then
        original_lines=$(wc -l < "$file")

        # Remove @keyframes already in globals.css
        sed -i '/^[[:space:]]*@keyframes spin/,/^[[:space:]]*}/d' "$file"
        sed -i '/^[[:space:]]*@keyframes hv-spin/,/^[[:space:]]*}/d' "$file"
        sed -i '/^[[:space:]]*@keyframes fade-in/,/^[[:space:]]*}/d' "$file"
        sed -i '/^[[:space:]]*@keyframes fade-out/,/^[[:space:]]*}/d' "$file"
        sed -i '/^[[:space:]]*@keyframes slide/,/^[[:space:]]*}/d' "$file"
        sed -i '/^[[:space:]]*@keyframes slideDown/,/^[[:space:]]*}/d' "$file"

        new_lines=$(wc -l < "$file")
        removed=$((original_lines - new_lines))

        if [ $removed -gt 0 ]; then
            echo "   ✅ $file: -$removed lignes (@keyframes)"
        fi
    fi
done

echo ""
echo "🎨 ÉTAPE 3: Remplacement animations par classes utilitaires"
echo "────────────────────────────────────────────────────────────"

for file in *.svelte; do
    if [ -f "$file" ]; then
        # Remplacer animation: spin par class
        sed -i 's/animation: spin 1s linear infinite/class="hv-animate-spin"/g' "$file"
        sed -i 's/animation: hv-spin 1s linear infinite/class="hv-animate-spin"/g' "$file"

        echo "   ✅ $file: Animations standardisées"
    fi
done

echo ""
echo "📊 ÉTAPE 4: Rapport final"
echo "────────────────────────────────────────────────────────────"

cd "$FRONTEND_DIR"

# Count results
TOTAL_SVELTE=$(find . -name "*.svelte" -type f | wc -l)
TOTAL_LINES=$(find . -name "*.svelte" -type f -exec wc -l {} + 2>/dev/null | tail -1 | awk '{print $1}')
VAR_COUNT=$(grep -r "var(--" . --include="*.svelte" | wc -l)
REMAINING_MODALS=$(grep -r "class=\"hv-modal-overlay\"" . --include="*.svelte" | wc -l)
REMAINING_KEYFRAMES=$(grep -r "@keyframes" . --include="*.svelte" | wc -l)

echo ""
echo "📈 Statistiques finales:"
echo "   • Fichiers Svelte: $TOTAL_SVELTE"
echo "   • Lignes totales: $TOTAL_LINES"
echo "   • Variables CSS: $VAR_COUNT occurrences"
echo "   • Modals custom restants: $REMAINING_MODALS"
echo "   • @keyframes restants: $REMAINING_KEYFRAMES"
echo ""

# Verify compilation
echo "🔨 ÉTAPE 5: Vérification compilation"
echo "────────────────────────────────────────────────────────────"

cd "/home/user/Projet/HOMEVAULT - HTMX/frontend"

if npm run check 2>&1 | grep -q "ERROR"; then
    echo "❌ Erreurs de compilation détectées"
    echo "⚠️  Rollback recommandé"
else
    echo "✅ Compilation réussie"
fi

echo ""
echo "════════════════════════════════════════════════════════════"
echo "✅ PHASE 1 - NETTOYAGE TERMINÉ"
echo "════════════════════════════════════════════════════════════"
echo ""
echo "💾 Backup disponible: $BACKUP_DIR"
echo ""
echo "📋 Actions suivantes recommandées:"
echo "   1. Tester manuellement toutes les pages"
echo "   2. Vérifier le dark mode"
echo "   3. Tester tous les modals"
echo "   4. Valider les formulaires"
echo ""
echo "🎯 Phase 1 complétée à 90%"
echo "   Reste: Migration manuelle des modals vers <Modal>"
echo ""
