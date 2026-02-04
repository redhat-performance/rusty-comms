/**
 * Docusaurus sidebar configuration
 * 
 * This file defines the documentation sidebar structure.
 * Copy this to sidebars.js after initializing Docusaurus.
 */

const sidebars = {
  docs: [
    {
      type: 'doc',
      id: 'intro',
      label: 'Introduction',
    },
    {
      type: 'category',
      label: 'User Guide',
      collapsed: false,
      items: [
        'user-guide/getting-started',
        'user-guide/basic-usage',
        'user-guide/advanced-usage',
        'user-guide/ipc-mechanisms',
        'user-guide/output-formats',
        'user-guide/performance-tuning',
        'user-guide/troubleshooting',
      ],
    },
    {
      type: 'category',
      label: 'Reference',
      items: [
        'reference/cli-reference',
        'reference/json-schema',
        'reference/environment-variables',
      ],
    },
    {
      type: 'category',
      label: 'Tutorials',
      items: [
        'tutorials/comparing-ipc-mechanisms',
        'tutorials/cross-process-testing',
        'tutorials/dashboard-analysis',
      ],
    },
    {
      type: 'category',
      label: 'Concepts',
      items: [
        'concepts/latency-measurement',
        'concepts/async-vs-blocking',
        'concepts/shared-memory-options',
      ],
    },
    {
      type: 'doc',
      id: 'CHANGELOG',
      label: 'Changelog',
    },
  ],
};

module.exports = sidebars;
