#!/usr/bin/env node

/**
 * Architecture Analyzer Script
 *
 * Analyzes file paths and import edges to compute structural patterns
 * that inform architectural layer identification.
 */

const fs = require('fs');
const path = require('path');

// Main analysis function
function analyzeGraph(inputPath, outputPath) {
  const input = JSON.parse(fs.readFileSync(inputPath, 'utf8'));
  const { fileNodes, importEdges, allEdges } = input;

  // Initialize results
  const results = {
    scriptCompleted: true,
    directoryGroups: {},
    nodeTypeGroups: {
      file: [],
      config: [],
      document: [],
      service: [],
      pipeline: [],
      table: [],
      schema: [],
      resource: [],
      endpoint: []
    },
    crossCategoryEdges: [],
    interGroupImports: [],
    intraGroupDensity: {},
    patternMatches: {},
    deploymentTopology: {
      hasDockerfile: false,
      hasCompose: false,
      hasK8s: false,
      hasTerraform: false,
      hasCI: false,
      infraFiles: []
    },
    dataPipeline: {
      schemaFiles: [],
      migrationFiles: [],
      dataModelFiles: [],
      apiHandlerFiles: []
    },
    docCoverage: {
      groupsWithDocs: 0,
      totalGroups: 0,
      coverageRatio: 0,
      undocumentedGroups: []
    },
    dependencyDirection: [],
    fileStats: {
      totalFileNodes: fileNodes.length,
      filesPerGroup: {},
      nodeTypeCounts: {}
    },
    fileFanIn: {},
    fileFanOut: {}
  };

  // Directory pattern matching table
  const directoryPatterns = {
    'routes': 'api',
    'api': 'api',
    'controllers': 'api',
    'endpoints': 'api',
    'handlers': 'api',
    'services': 'service',
    'core': 'service',
    'lib': 'service',
    'domain': 'service',
    'logic': 'service',
    'models': 'data',
    'db': 'data',
    'data': 'data',
    'persistence': 'data',
    'repository': 'data',
    'entities': 'data',
    'components': 'ui',
    'views': 'ui',
    'pages': 'ui',
    'ui': 'ui',
    'layouts': 'ui',
    'screens': 'ui',
    'renderer': 'ui',
    'middleware': 'middleware',
    'plugins': 'middleware',
    'interceptors': 'middleware',
    'guards': 'middleware',
    'utils': 'utility',
    'helpers': 'utility',
    'common': 'utility',
    'shared': 'utility',
    'tools': 'utility',
    'config': 'config',
    'constants': 'config',
    'env': 'config',
    'settings': 'config',
    '__tests__': 'test',
    'test': 'test',
    'tests': 'test',
    'spec': 'test',
    'specs': 'test',
    'types': 'types',
    'interfaces': 'types',
    'schemas': 'types',
    'contracts': 'types',
    'dtos': 'types',
    'hooks': 'hooks',
    'store': 'state',
    'state': 'state',
    'reducers': 'state',
    'actions': 'state',
    'slices': 'state',
    'assets': 'assets',
    'static': 'assets',
    'public': 'assets',
    'migrations': 'data',
    'management': 'config',
    'templatetags': 'utility',
    'signals': 'service',
    'serializers': 'api',
    'cmd': 'entry',
    'internal': 'service',
    'pkg': 'utility',
    'dto': 'types',
    'request': 'types',
    'response': 'types',
    'entity': 'data',
    'controller': 'api',
    'routers': 'api',
    'composables': 'service',
    'blueprints': 'api',
    'mailers': 'service',
    'jobs': 'service',
    'channels': 'service',
    'bin': 'entry',
    'docs': 'documentation',
    'documentation': 'documentation',
    'wiki': 'documentation',
    'deploy': 'infrastructure',
    'deployment': 'infrastructure',
    'infra': 'infrastructure',
    'infrastructure': 'infrastructure',
    '.github': 'ci-cd',
    '.gitlab': 'ci-cd',
    '.circleci': 'ci-cd',
    'k8s': 'infrastructure',
    'kubernetes': 'infrastructure',
    'helm': 'infrastructure',
    'charts': 'infrastructure',
    'terraform': 'infrastructure',
    'tf': 'infrastructure',
    'docker': 'infrastructure',
    'sql': 'data',
    'database': 'data',
    'schema': 'data',
    'electron': 'entry',
    'ble': 'service',
    'native': 'service',
    'scripts': 'infrastructure',
    'artifacts': 'infrastructure',
    'crates': 'service',
    'workflows': 'ci-cd'
  };

  // File-level pattern matching
  function matchFilePattern(filePath, fileName) {
    // Test files
    if (/\.(test|spec)\.[jt]sx?$/.test(fileName) ||
        /test_.*\.py$/.test(fileName) ||
        /_test\.go$/.test(fileName) ||
        /Test\.java$/.test(fileName) ||
        /_spec\.rb$/.test(fileName) ||
        /Test\.php$/.test(fileName) ||
        /Tests\.cs$/.test(fileName)) {
      return 'test';
    }

    // TypeScript declaration files (only .d.ts, not regular .ts files)
    if (/\.d\.ts$/.test(fileName)) {
      return 'types';
    }

    // Entry point files
    const entryPatterns = [
      /^index\.(ts|js|tsx|jsx)$/,
      /^__init__\.py$/,
      /^manage\.py$/,
      /^main\.go$/,
      /^main\.rs$/,
      /^lib\.rs$/,
      /^Application\.java$/,
      /^Program\.cs$/,
      /^config\.ru$/
    ];

    const filePathWithoutExt = filePath.replace(/\.(ts|js|tsx|jsx|py|go|rs|java|cs|rb)$/, '');
    const fileNameWithoutExt = fileName.replace(/\.(ts|js|tsx|jsx|py|go|rs|java|cs|rb)$/, '');

    if (entryPatterns.some(p => p.test(fileNameWithoutExt))) {
      // Only mark as entry if it's at a package root or significant location
      const dirParts = filePath.split('/').filter(p => p);
      if (dirParts.length <= 2 || dirParts.includes('src') ||
          dirParts.includes('crates') || dirParts.includes('bin')) {
        return 'entry';
      }
    }

    // Config files
    const configPatterns = [
      /^Cargo\.toml$/,
      /^go\.mod$/,
      /^Gemfile$/,
      /^pom\.xml$/,
      /^build\.gradle/,
      /^composer\.json$/,
      /^Dockerfile$/,
      /^docker-compose/,
      /\.tf$/,
      /\.tfvars$/,
      /^Makefile$/,
      /^package\.json$/,
      /^tsconfig\.json$/,
      /vite\.config\./,
      /vitest\.config\./,
      /\.eslintrc/,
      /eslint\.config\./,
      /\.prettierrc/,
      /\.nvmrc$/,
      /^\.env/,
      /\.config\.(js|ts|mjs|cjs|json)$/,
      /^electron-builder/
    ];

    if (configPatterns.some(p => p.test(fileName))) {
      return 'config';
    }

    // CI/CD files
    if (/\.github\/workflows\//.test(filePath) ||
        /\.gitlab-ci\.yml$/.test(fileName) ||
        /^Jenkinsfile$/.test(fileName) ||
        /\.circleci\//.test(filePath)) {
      return 'ci-cd';
    }

    // Infrastructure files
    if (/Dockerfile/.test(fileName) ||
        /docker-compose/.test(fileName) ||
        /\.tf$/.test(fileName) ||
        /\.tfvars$/.test(fileName) ||
        /^Makefile$/.test(fileName) ||
        fileName.startsWith('k8s') ||
        filePath.includes('kubernetes') ||
        filePath.includes('terraform') ||
        filePath.includes('deploy')) {
      return 'infrastructure';
    }

    // Data files
    if (/\.sql$/.test(fileName) ||
        /migrations\//.test(filePath) ||
        fileName.includes('migration') ||
        fileName.includes('schema')) {
      return 'data';
    }

    // Type/contract files
    if (/\.graphql$/.test(fileName) ||
        /\.gql$/.test(fileName) ||
        /\.proto$/.test(fileName)) {
      return 'types';
    }

    // Documentation files
    if (/\.md$/.test(fileName) ||
        /\.rst$/.test(fileName)) {
      return 'documentation';
    }

    return null;
  }

  // A. Directory Grouping
  function computeCommonPrefix(filePaths) {
    if (filePaths.length === 0) return '';

    const parts = filePaths[0].split('/');
    let commonLength = parts.length;

    for (let i = 1; i < filePaths.length && commonLength > 0; i++) {
      const currentParts = filePaths[i].split('/');
      let j = 0;
      while (j < commonLength && j < currentParts.length && parts[j] === currentParts[j]) {
        j++;
      }
      commonLength = j;
    }

    return parts.slice(0, commonLength).join('/') + (commonLength > 0 ? '/' : '');
  }

  const filePaths = fileNodes.map(n => n.filePath);
  const commonPrefix = computeCommonPrefix(filePaths);

  // Group by directory
  for (const node of fileNodes) {
    const type = node.type;
    results.nodeTypeGroups[type].push(node.id);

    // Directory grouping
    let dirGroup;
    let relativePath = node.filePath;

    if (commonPrefix && relativePath.startsWith(commonPrefix)) {
      relativePath = relativePath.substring(commonPrefix.length);
    }

    const pathParts = relativePath.split('/').filter(p => p && p !== '.');

    if (pathParts.length === 0) {
      dirGroup = 'root';
    } else {
      dirGroup = pathParts[0];
    }

    if (!results.directoryGroups[dirGroup]) {
      results.directoryGroups[dirGroup] = [];
    }
    results.directoryGroups[dirGroup].push(node.id);

    // Pattern matching
    let pattern = directoryPatterns[dirGroup.toLowerCase()];
    if (!pattern) {
      const filePattern = matchFilePattern(node.filePath, node.name);
      if (filePattern) {
        pattern = filePattern;
      }
    }
    if (pattern) {
      results.patternMatches[dirGroup] = pattern;
    }
  }

  // Count node types
  for (const type of Object.keys(results.nodeTypeGroups)) {
    results.fileStats.nodeTypeCounts[type] = results.nodeTypeGroups[type].length;
  }

  // Count files per group
  for (const group of Object.keys(results.directoryGroups)) {
    results.fileStats.filesPerGroup[group] = results.directoryGroups[group].length;
  }

  // B. Import Adjacency Matrix
  const adjacency = {};
  const fanIn = {};
  const fanOut = {};

  for (const node of fileNodes) {
    adjacency[node.id] = { imports: [], importedBy: [] };
    fanIn[node.id] = 0;
    fanOut[node.id] = 0;
  }

  for (const edge of importEdges) {
    if (adjacency[edge.source] && adjacency[edge.target]) {
      adjacency[edge.source].imports.push(edge.target);
      adjacency[edge.target].importedBy.push(edge.source);
      fanOut[edge.source]++;
      fanIn[edge.target]++;
    }
  }

  results.fileFanIn = fanIn;
  results.fileFanOut = fanOut;

  // C. Cross-Category Dependency Analysis
  const crossCategoryCounts = {};
  for (const edge of allEdges) {
    const sourceNode = fileNodes.find(n => n.id === edge.source);
    const targetNode = fileNodes.find(n => n.id === edge.target);

    if (sourceNode && targetNode && sourceNode.type !== targetNode) {
      const key = `${sourceNode.type}->${targetNode.type}:${edge.type}`;
      crossCategoryCounts[key] = (crossCategoryCounts[key] || 0) + 1;
    }
  }

  for (const [key, count] of Object.entries(crossCategoryCounts).sort((a, b) => b[1] - a[1])) {
    const [fromType, toTypeWithEdge] = key.split(':');
    const toType = toTypeWithEdge;
    results.crossCategoryEdges.push({ fromType, toType, edgeType: key.split(':')[1], count });
  }

  // D. Inter-Group Import Frequency
  const groupImportMap = {};

  for (const edge of importEdges) {
    const sourceNode = fileNodes.find(n => n.id === edge.source);
    const targetNode = fileNodes.find(n => n.id === edge.target);

    if (sourceNode && targetNode) {
      const sourceGroup = getGroupForFile(sourceNode.id, results.directoryGroups);
      const targetGroup = getGroupForFile(targetNode.id, results.directoryGroups);

      if (sourceGroup && targetGroup && sourceGroup !== targetGroup) {
        const key = `${sourceGroup}->${targetGroup}`;
        if (!groupImportMap[key]) {
          groupImportMap[key] = { from: sourceGroup, to: targetGroup, count: 0 };
        }
        groupImportMap[key].count++;
      }
    }
  }

  results.interGroupImports = Object.values(groupImportMap).sort((a, b) => b.count - a.count);

  // E. Intra-Group Import Density
  const groupInternalEdges = {};
  const groupTotalEdges = {};

  for (const group of Object.keys(results.directoryGroups)) {
    groupInternalEdges[group] = 0;
    groupTotalEdges[group] = 0;
  }

  for (const edge of importEdges) {
    const sourceNode = fileNodes.find(n => n.id === edge.source);
    const targetNode = fileNodes.find(n => n.id === edge.target);

    if (sourceNode && targetNode) {
      const sourceGroup = getGroupForFile(sourceNode.id, results.directoryGroups);
      const targetGroup = getGroupForFile(targetNode.id, results.directoryGroups);

      if (sourceGroup) {
        groupTotalEdges[sourceGroup]++;
      }
      if (targetGroup) {
        groupTotalEdges[targetGroup]++;
      }

      if (sourceGroup && targetGroup && sourceGroup === targetGroup) {
        groupInternalEdges[sourceGroup]++;
      }
    }
  }

  for (const group of Object.keys(results.directoryGroups)) {
    const internal = groupInternalEdges[group] || 0;
    const total = groupTotalEdges[group] || 0;
    const density = total > 0 ? internal / total : 0;
    results.intraGroupDensity[group] = {
      internalEdges: internal,
      totalEdges: total,
      density: density
    };
  }

  // F. Deployment Topology Detection
  const deploymentFiles = fileNodes.filter(n =>
    n.filePath.includes('Dockerfile') ||
    n.filePath.includes('docker-compose') ||
    n.filePath.includes('.github') ||
    n.filePath.includes('k8s') ||
    n.filePath.includes('terraform') ||
    n.filePath.includes('deploy') ||
    n.name.includes('Dockerfile') ||
    n.name.includes('release')
  );

  results.deploymentTopology.hasCI = deploymentFiles.some(n =>
    n.filePath.includes('.github') || n.filePath.includes('workflow') || n.name.includes('release'));
  results.deploymentTopology.hasDockerfile = deploymentFiles.some(n =>
    n.name.includes('Dockerfile') || n.filePath.includes('Dockerfile'));
  results.deploymentTopology.hasCompose = deploymentFiles.some(n =>
    n.name.includes('docker-compose') || n.filePath.includes('docker-compose'));
  results.deploymentTopology.hasK8s = deploymentFiles.some(n =>
    n.filePath.includes('k8s') || n.filePath.includes('kubernetes'));
  results.deploymentTopology.hasTerraform = deploymentFiles.some(n =>
    n.filePath.includes('terraform') || n.name.endsWith('.tf'));
  results.deploymentTopology.infraFiles = deploymentFiles.map(n => n.filePath);

  // G. Documentation Coverage
  const allGroups = Object.keys(results.directoryGroups);
  const docFiles = fileNodes.filter(n => n.type === 'document' || n.filePath.endsWith('.md'));
  const groupsWithDocs = new Set();

  for (const doc of docFiles) {
    const docGroup = getGroupForFile(doc.id, results.directoryGroups);
    if (docGroup) {
      groupsWithDocs.add(docGroup);
    }
  }

  results.docCoverage.totalGroups = allGroups.length;
  results.docCoverage.groupsWithDocs = groupsWithDocs.size;
  results.docCoverage.coverageRatio = allGroups.length > 0 ? groupsWithDocs.size / allGroups.length : 0;
  results.docCoverage.undocumentedGroups = allGroups.filter(g => !groupsWithDocs.has(g));

  // H. Dependency Direction
  const processedPairs = new Set();
  for (const [key, data] of Object.entries(groupImportMap)) {
    const reverseKey = `${data.to}->${data.from}`;
    if (processedPairs.has(key) || processedPairs.has(reverseKey)) continue;

    const reverse = groupImportMap[reverseKey];
    if (reverse) {
      if (data.count > reverse.count) {
        results.dependencyDirection.push({ dependent: data.from, dependsOn: data.to, strength: data.count - reverse.count });
      } else if (reverse.count > data.count) {
        results.dependencyDirection.push({ dependent: data.to, dependsOn: data.from, strength: reverse.count - data.count });
      } else {
        // Equal - bidirectional
        results.dependencyDirection.push({ dependent: data.from, dependsOn: data.to, strength: data.count, bidirectional: true });
      }
    } else {
      results.dependencyDirection.push({ dependent: data.from, dependsOn: data.to, strength: data.count });
    }

    processedPairs.add(key);
    processedPairs.add(reverseKey);
  }

  // Write results
  fs.writeFileSync(outputPath, JSON.stringify(results, null, 2));
  return 0;
}

// Helper function to get group for a file
function getGroupForFile(fileId, directoryGroups) {
  for (const [group, files] of Object.entries(directoryGroups)) {
    if (files.includes(fileId)) {
      return group;
    }
  }
  return null;
}

// Main execution
if (require.main === module) {
  const inputPath = process.argv[2];
  const outputPath = process.argv[3];

  if (!inputPath || !outputPath) {
    console.error('Usage: node ua-arch-analyze.js <input.json> <output.json>');
    process.exit(1);
  }

  try {
    const exitCode = analyzeGraph(inputPath, outputPath);
    process.exit(exitCode);
  } catch (error) {
    console.error('Error:', error.message);
    process.exit(1);
  }
}

module.exports = { analyzeGraph };
