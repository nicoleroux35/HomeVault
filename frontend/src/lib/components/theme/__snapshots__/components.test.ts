/**
 * Snapshot tests for core UI components
 * Verifies component structure and rendered output remain stable
 *
 * `Button`, `Alert` and `Modal` are Svelte 5 runes components, so their content
 * arrives as a `children` **snippet**, never as a string prop — passing a
 * string makes Svelte throw "snippet is not a function". `createRawSnippet`
 * builds a minimal text snippet for them. `Badge` is still a legacy
 * `export let` / `<slot />` component, so a plain string fills its slot.
 */

import { render } from '@testing-library/svelte';
import { createRawSnippet } from 'svelte';
import { describe, it, expect } from 'vitest';
import Button from '$lib/components/theme/Button.svelte';
import Badge from '$lib/components/theme/Badge.svelte';
import Alert from '$lib/components/theme/Alert.svelte';
import Modal from '$lib/components/theme/Modal.svelte';

/** A text-only snippet, the shape every runes component here expects. */
const text = (value: string) => createRawSnippet(() => ({ render: () => value }));

describe('Button component snapshots', () => {
  it('renders default button', () => {
    const { container } = render(Button, {
      props: { children: text('Click me') },
    });
    expect(container.innerHTML).toMatchSnapshot();
  });

  it('renders primary button', () => {
    const { container } = render(Button, {
      props: { variant: 'primary', children: text('Primary') },
    });
    expect(container.innerHTML).toMatchSnapshot();
  });

  it('renders disabled button', () => {
    const { container } = render(Button, {
      props: { disabled: true, children: text('Disabled') },
    });
    expect(container.innerHTML).toMatchSnapshot();
  });

  it('renders small button', () => {
    const { container } = render(Button, {
      props: { size: 'sm', children: text('Small') },
    });
    expect(container.innerHTML).toMatchSnapshot();
  });

  it('renders button with icon', () => {
    const { container } = render(Button, {
      props: { children: text('With Icon'), icon: 'folder' },
    });
    expect(container.innerHTML).toMatchSnapshot();
  });
});

describe('Badge component snapshots', () => {
  it('renders default badge', () => {
    const { container } = render(Badge, {
      props: { children: 'Badge' },
    });
    expect(container.innerHTML).toMatchSnapshot();
  });

  it('renders success badge', () => {
    const { container } = render(Badge, {
      props: { variant: 'success', children: 'Success' },
    });
    expect(container.innerHTML).toMatchSnapshot();
  });

  it('renders warning badge', () => {
    const { container } = render(Badge, {
      props: { variant: 'warning', children: 'Warning' },
    });
    expect(container.innerHTML).toMatchSnapshot();
  });

  it('renders error badge', () => {
    const { container } = render(Badge, {
      props: { variant: 'error', children: 'Error' },
    });
    expect(container.innerHTML).toMatchSnapshot();
  });
});

describe('Alert component snapshots', () => {
  it('renders info alert', () => {
    const { container } = render(Alert, {
      props: {
        variant: 'info',
        title: 'Information',
        children: text('This is an info message'),
      },
    });
    expect(container.innerHTML).toMatchSnapshot();
  });

  it('renders success alert', () => {
    const { container } = render(Alert, {
      props: {
        variant: 'success',
        title: 'Success',
        children: text('Operation completed successfully'),
      },
    });
    expect(container.innerHTML).toMatchSnapshot();
  });

  it('renders warning alert', () => {
    const { container } = render(Alert, {
      props: {
        variant: 'warning',
        title: 'Warning',
        children: text('Please review this action'),
      },
    });
    expect(container.innerHTML).toMatchSnapshot();
  });

  it('renders error alert', () => {
    const { container } = render(Alert, {
      props: {
        variant: 'error',
        title: 'Error',
        children: text('An error occurred'),
      },
    });
    expect(container.innerHTML).toMatchSnapshot();
  });

  it('renders dismissible alert', () => {
    const { container } = render(Alert, {
      props: {
        variant: 'info',
        dismissible: true,
        children: text('Dismissible message'),
      },
    });
    expect(container.innerHTML).toMatchSnapshot();
  });
});

describe('Modal component snapshots', () => {
  it('renders closed modal', () => {
    const { container } = render(Modal, {
      props: {
        open: false,
        title: 'Modal Title',
        children: text('Modal content'),
      },
    });
    expect(container.innerHTML).toMatchSnapshot();
  });

  it('renders open modal', () => {
    const { container } = render(Modal, {
      props: {
        open: true,
        title: 'Modal Title',
        children: text('Modal content'),
      },
    });
    expect(container.innerHTML).toMatchSnapshot();
  });

  it('renders modal with footer', () => {
    const { container } = render(Modal, {
      props: {
        open: true,
        title: 'Confirm Action',
        children: text('Are you sure?'),
        footer: text('Footer actions'),
      },
    });
    expect(container.innerHTML).toMatchSnapshot();
  });
});
