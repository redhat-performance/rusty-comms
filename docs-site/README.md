# Docusaurus Documentation Site

This directory contains the configuration for a Docusaurus documentation website.

## Setup

```bash
# Prerequisites: Node.js 18+ and npm

# Create the Docusaurus project
npx create-docusaurus@latest . classic --typescript

# Or initialize from existing docs
npx create-docusaurus@latest . classic
```

## Configuration

After initialization, update `docusaurus.config.js`:

```javascript
module.exports = {
  title: 'Rusty-Comms',
  tagline: 'IPC Benchmark Suite Documentation',
  url: 'https://redhat-performance.github.io',
  baseUrl: '/rusty-comms/',
  organizationName: 'redhat-performance',
  projectName: 'rusty-comms',
  
  presets: [
    [
      'classic',
      {
        docs: {
          sidebarPath: require.resolve('./sidebars.js'),
          editUrl: 'https://github.com/redhat-performance/rusty-comms/edit/main/',
        },
      },
    ],
  ],
  
  themeConfig: {
    navbar: {
      title: 'Rusty-Comms',
      items: [
        {type: 'doc', docId: 'intro', position: 'left', label: 'Docs'},
        {href: 'https://github.com/redhat-performance/rusty-comms', label: 'GitHub', position: 'right'},
      ],
    },
    algolia: {
      // Algolia search configuration (requires account)
      appId: 'YOUR_APP_ID',
      apiKey: 'YOUR_SEARCH_API_KEY',
      indexName: 'rusty-comms',
    },
  },
};
```

## Directory Structure

After setup:

```
docs-site/
├── docs/               # Copy docs from ../docs/
│   ├── intro.md
│   ├── user-guide/
│   └── reference/
├── src/
│   ├── components/
│   └── pages/
├── static/
├── docusaurus.config.js
├── sidebars.js
└── package.json
```

## Development

```bash
# Start development server
npm start

# Build for production
npm run build

# Serve production build locally
npm run serve
```

## Importing Documentation

Copy the Markdown documentation:

```bash
# From project root
cp -r docs/* docs-site/docs/
```

## Versioning

Docusaurus supports documentation versioning:

```bash
# Create a new version
npm run docusaurus docs:version 1.0

# Versions are stored in versioned_docs/
```

## Algolia Search Setup

1. Create an Algolia account at https://www.algolia.com
2. Create an application and search index
3. Update `docusaurus.config.js` with your credentials
4. Configure the crawler to index your documentation

## Deployment

### GitHub Pages

```bash
# Set environment variables
export GIT_USER=<github-username>
export USE_SSH=true

# Deploy
npm run deploy
```

### Vercel/Netlify

Connect your repository and configure:
- Build command: `npm run build`
- Output directory: `build`

## See Also

- [Docusaurus Documentation](https://docusaurus.io/docs)
- [Project Documentation](../docs/README.md)
