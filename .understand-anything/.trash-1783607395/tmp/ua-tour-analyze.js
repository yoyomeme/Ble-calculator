#!/usr/bin/env node

const fs = require('fs');
const path = require('path');

const INPUT_FILE = process.argv[2];
const OUTPUT_FILE = process.argv[3];

if (!INPUT_FILE || !OUTPUT_FILE) {
  console.error('Usage: node ua-tour-analyze.js <input.json> <output.json>');
  process.exit(1);
}

let inputData;
try {
  inputData = JSON.parse(fs.readFileSync(INPUT_FILE, 'utf8'));
} catch (err) {
  console.error(`Failed to read input file: ${err.message}`);
  process.exit(1);
}

const { nodes, edges, layers } = inputData;

// === A. Fan-In Ranking (Importance) ===
const fanInMap = {};
nodes.forEach(n => fanInMap[n.id] = 0);
edges.forEach(e => {
  if (fanInMap[e.target] !== undefined) {
    fanInMap[e.target]++;
  }
});

const fanInRanking = Object.entries(fanInMap)
  .map(([id, fanIn]) => {
    const node = nodes.find(n => n.id === id);
    return { id, fanIn, name: node?.name || id, type: node?.type || 'unknown' };
  })
  .sort((a, b) => b.fanIn - a.fanIn)
  .slice(0, 20);

// === B. Fan-Out Ranking (Scope) ===
const fanOutMap = {};
nodes.forEach(n => fanOutMap[n.id] = 0);
edges.forEach(e => {
  if (fanOutMap[e.source] !== undefined) {
    fanOutMap[e.source]++;
  }
});

const fanOutRanking = Object.entries(fanOutMap)
  .map(([id, fanOut]) => {
    const node = nodes.find(n => n.id === id);
    return { id, fanOut, name: node?.name || id, type: node?.type || 'unknown' };
  })
  .sort((a, b) => b.fanOut - a.fanOut)
  .slice(0, 20);

// === C. Entry Point Candidates ===
const entryPointScores = {};
nodes.forEach(n => entryPointScores[n.id] = 0);

const codeEntryPatterns = [
  'index.ts', 'index.js', 'main.ts', 'main.js', 'app.ts', 'app.js',
  'server.ts', 'server.js', 'mod.rs', 'main.go', 'main.py', 'main.rs',
  'manage.py', 'app.py', 'wsgi.py', 'asgi.py', 'run.py', '__main__.py',
  'Application.java', 'Main.java', 'Program.cs', 'config.ru', 'index.php',
  'App.swift', 'Application.kt', 'main.cpp', 'main.c'
];

const top10PercentFanOut = Math.ceil(...Object.values(fanOutMap).sort((a, b) => b - a).slice(0, Math.max(1, Math.floor(nodes.length * 0.1))));
const bottom25PercentFanIn = Object.values(fanInMap).sort((a, b) => a - b)[Math.floor(nodes.length * 0.25)] || 0;

nodes.forEach(n => {
  const name = n.name || '';
  const filePath = n.filePath || '';

  // Code files
  if (n.type === 'file') {
    if (codeEntryPatterns.some(p => name === p || name.endsWith('/' + p))) {
      entryPointScores[n.id] += 3;
    }
    if (filePath.split('/').length <= 2) {
      entryPointScores[n.id] += 1;
    }
    if (fanOutMap[n.id] >= top10PercentFanOut) {
      entryPointScores[n.id] += 1;
    }
    if (fanInMap[n.id] <= bottom25PercentFanIn) {
      entryPointScores[n.id] += 1;
    }
  }

  // Documentation files
  if (n.type === 'document') {
    if (name === 'README.md' && filePath.split('/').length === 1) {
      entryPointScores[n.id] += 5;
    } else if (name.endsWith('.md') && filePath.split('/').length === 1) {
      entryPointScores[n.id] += 2;
    }
  }
});

const entryPointCandidates = Object.entries(entryPointScores)
  .filter(([id, score]) => score > 0)
  .map(([id, score]) => {
    const node = nodes.find(n => n.id === id);
    return { id, score, name: node?.name || id, summary: node?.summary || '' };
  })
  .sort((a, b) => b.score - a.score)
  .slice(0, 5);

// === D. Dependency Chains (BFS from Entry Point) ===
// Find top code entry point (skip documentation)
const codeEntryPoints = entryPointCandidates.filter(e => {
  const node = nodes.find(n => n.id === e.id);
  return node && node.type === 'file';
});

const startNode = codeEntryPoints.length > 0 ? codeEntryPoints[0].id : null;

const bfsOrder = [];
const depthMap = {};
const byDepth = {};
const visited = new Set();

if (startNode) {
  const queue = [[startNode, 0]];
  visited.add(startNode);

  while (queue.length > 0) {
    const [nodeId, depth] = queue.shift();
    bfsOrder.push(nodeId);
    depthMap[nodeId] = depth;

    if (!byDepth[depth]) byDepth[depth] = [];
    byDepth[depth].push(nodeId);

    // Follow imports and calls edges forward
    edges
      .filter(e => e.source === nodeId && (e.type === 'imports' || e.type === 'calls'))
      .forEach(e => {
        if (!visited.has(e.target)) {
          visited.add(e.target);
          queue.push([e.target, depth + 1]);
        }
      });
  }
}

// === E. Non-Code File Inventory ===
const nonCodeFiles = {
  documentation: [],
  infrastructure: [],
  data: [],
  config: []
};

nodes.forEach(n => {
  if (n.type === 'document') {
    nonCodeFiles.documentation.push({
      id: n.id,
      name: n.name,
      summary: n.summary
    });
  } else if (n.type === 'service' || n.type === 'pipeline' || n.type === 'resource') {
    nonCodeFiles.infrastructure.push({
      id: n.id,
      name: n.name,
      summary: n.summary
    });
  } else if (n.type === 'table' || n.type === 'schema' || n.type === 'endpoint') {
    nonCodeFiles.data.push({
      id: n.id,
      name: n.name,
      summary: n.summary
    });
  } else if (n.type === 'config') {
    nonCodeFiles.config.push({
      id: n.id,
      name: n.name,
      summary: n.summary
    });
  }
});

// === F. Tightly Coupled Clusters ===
const adjacencyList = {};
nodes.forEach(n => adjacencyList[n.id] = new Set());

edges.forEach(e => {
  if (adjacencyList[e.source]) adjacencyList[e.source].add(e.target);
  if (adjacencyList[e.target]) adjacencyList[e.target].add(e.source);
});

const clusters = [];
const inCluster = new Set();

nodes.forEach(n => {
  if (inCluster.has(n.id)) return;

  const neighbors = Array.from(adjacencyList[n.id] || []);
  const clusterNodes = [n.id];

  // Find bidirectional relationships
  neighbors.forEach(neighborId => {
    if (adjacencyList[neighborId] && adjacencyList[neighborId].has(n.id)) {
      if (!clusterNodes.includes(neighborId)) {
        clusterNodes.push(neighborId);
      }
    }
  });

  // Expand cluster: nodes connected to 2+ cluster members
  let added = true;
  while (added && clusterNodes.length < 5) {
    added = false;
    nodes.forEach(candidate => {
      if (clusterNodes.includes(candidate.id) || inCluster.has(candidate.id)) return;

      const connections = clusterNodes.filter(clusterId =>
        adjacencyList[candidate.id] && adjacencyList[candidate.id].has(clusterId)
      ).length;

      if (connections >= 2) {
        clusterNodes.push(candidate.id);
        added = true;
      }
    });
  }

  if (clusterNodes.length >= 2) {
    const edgeCount = clusterNodes.reduce((count, nodeId) => {
      const nodeEdges = edges.filter(e =>
        e.source === nodeId && clusterNodes.includes(e.target)
      );
      return count + nodeEdges.length;
    }, 0);

    clusters.push({ nodes: clusterNodes, edgeCount });
    clusterNodes.forEach(id => inCluster.add(id));
  }
});

clusters.sort((a, b) => b.nodes.length - a.nodes.length);
const topClusters = clusters.slice(0, 10);

// === G. Layer List ===
const layersInfo = {
  count: layers.length,
  list: layers.map(l => ({
    id: l.id,
    name: l.name,
    description: l.description
  }))
};

// === H. Node Summary Index ===
const nodeSummaryIndex = {};
nodes.forEach(n => {
  nodeSummaryIndex[n.id] = {
    name: n.name,
    type: n.type,
    summary: n.summary
  };
});

// === Output ===
const output = {
  scriptCompleted: true,
  entryPointCandidates,
  fanInRanking,
  fanOutRanking,
  bfsTraversal: {
    startNode,
    order: bfsOrder,
    depthMap,
    byDepth
  },
  nonCodeFiles,
  clusters: topClusters,
  layers: layersInfo,
  nodeSummaryIndex,
  totalNodes: nodes.length,
  totalEdges: edges.length
};

fs.writeFileSync(OUTPUT_FILE, JSON.stringify(output, null, 2));
process.exit(0);
