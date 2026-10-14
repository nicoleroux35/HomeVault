/* =============================================================================
 * File          : index.ts
 * Author        : Nicolas Le Roux
 * Created       : 28/09/2026
 * Last modified : 28/09/2026 by Nicolas Le Roux
 * Version       : 1.0.0
 *
 * Description:
 *   Barrel export of the design-system components. The interactive
 *   primitives delegate to the vendored components/ui layer (shadcn-svelte /
 *   bits-ui); the legacy layout components were deleted once their last
 *   consumer migrated to the app/ layer.
 *
 * Dependencies:
 *   - none (re-exports only)
 *
 * Usage:
 *   `import { Button, Modal, ... } from '$lib/components/theme'`.
 * =============================================================================
 */

/**
 * HOMEVAULT DESIGN SYSTEM - Component Exports
 *
 * The canonical interactive primitives (Button, Input, Select, Toggle,
 * Checkbox, Radio, Alert, Badge, Spinner, Modal) delegate to the shadcn-svelte
 * `components/ui/*` layer (bits-ui / Melt UI). The legacy v2.0 layout
 * components (Card, CardHeader, CardContent, ProgressBar, Divider) were
 * deleted 2026-09-12 once the last consumer (Rescue) migrated to `app/`.
 */

export { default as Button } from './Button.svelte';
export { default as Input } from './Input.svelte';
export { default as Select } from './Select.svelte';
export { default as Toggle } from './Toggle.svelte';
export { default as Switch } from './Switch.svelte';
export { default as Checkbox } from './Checkbox.svelte';
export { default as Radio } from './Radio.svelte';
export { default as Alert } from './Alert.svelte';
export { default as Badge } from './Badge.svelte';
export { default as Spinner } from './Spinner.svelte';
export { default as Modal } from './Modal.svelte';
export { default as Icon } from './Icon.svelte';
export { default as UserAvatar } from './UserAvatar.svelte';
export { default as FolderPicker } from './FolderPicker.svelte';
export { default as PathField } from './PathField.svelte';
export { default as PathPickerField } from './PathPickerField.svelte';
export { default as FloatingPathPicker } from './FloatingPathPicker.svelte';
export { default as Toast } from './Toast.svelte';
export { default as Drawer } from './Drawer.svelte';