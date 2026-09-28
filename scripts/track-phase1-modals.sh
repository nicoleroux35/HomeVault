#!/bin/bash

# Modal migration verification script - Phase 1
echo "📊 VÉRIFICATION MIGRATIONS MODALS - PHASE 1"
echo "════════════════════════════════════════════════════════"
echo ""

cd "/home/user/Projet/HOMEVAULT - HTMX/frontend/src/routes"

# Count migrated modals (using the Modal component)
echo "✅ Modals migrés vers composant Modal:"
echo "────────────────────────────────────────────────────────"

USERS_MODALS=$(grep -c "<Modal" Users.svelte 2>/dev/null || echo 0)
GROUPS_MODALS=$(grep -c "<Modal" Groups.svelte 2>/dev/null || echo 0)
EXPLORER_MODALS=$(grep -c "<Modal" Explorer.svelte 2>/dev/null || echo 0)

echo "Users.svelte:    $USERS_MODALS modals"
echo "Groups.svelte:   $GROUPS_MODALS modals"
echo "Explorer.svelte: $EXPLORER_MODALS modals"

TOTAL_MIGRATED=$((USERS_MODALS + GROUPS_MODALS + EXPLORER_MODALS))

echo ""
echo "Total modals migrés: $TOTAL_MIGRATED"

# Compter les anciens modals (structure <div class="hv-modal-overlay">)
echo ""
echo "⏳ Anciens modals restants:"
echo "────────────────────────────────────────────────────────"

OLD_USERS=$(grep -c "class=\"hv-modal-overlay\"" Users.svelte 2>/dev/null || echo 0)
OLD_GROUPS=$(grep -c "class=\"hv-modal-overlay\"" Groups.svelte 2>/dev/null || echo 0)
OLD_EXPLORER=$(grep -c "class=\"modal-overlay\"" Explorer.svelte 2>/dev/null || echo 0)

echo "Users.svelte:    $OLD_USERS anciens"
echo "Groups.svelte:   $OLD_GROUPS anciens"
echo "Explorer.svelte: $OLD_EXPLORER anciens"

TOTAL_OLD=$((OLD_USERS + OLD_GROUPS + OLD_EXPLORER))

echo ""
echo "Total anciens: $TOTAL_OLD"

echo ""
echo "════════════════════════════════════════════════════════"

if [ $TOTAL_OLD -eq 0 ]; then
  echo "🎉 TOUS LES MODALS SONT MIGRÉS!"
  echo ""
  echo "✅ Phase 1 - Migrations Modals: 100%"
  echo "✅ Phase 1 globale: 100%"
else
  PERCENT=$((TOTAL_MIGRATED * 100 / (TOTAL_MIGRATED + TOTAL_OLD)))
  echo "📈 Progression: $TOTAL_MIGRATED/$((TOTAL_MIGRATED + TOTAL_OLD)) modals migrés ($PERCENT%)"
  echo ""
  echo "⏳ Restant à migrer: $TOTAL_OLD modals"
fi

echo ""
