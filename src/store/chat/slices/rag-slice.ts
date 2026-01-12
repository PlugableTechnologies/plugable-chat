import type { StateCreator } from 'zustand';
import { invoke } from '../../../lib/api';
import type { RagChunk, RagIndexResult, OperationStatus } from '../types';

// Slice needs access to operationStatus from OperationStatusSlice
interface RagSliceDeps {
    operationStatus: OperationStatus | null;
}

export interface RagSlice {
    // RAG (Retrieval Augmented Generation) State
    attachedPaths: string[];
    ragIndexedFiles: string[];
    isIndexingRag: boolean;
    isSearchingRag: boolean;
    ragChunkCount: number;
    addAttachment: (path: string) => Promise<void>;
    removeAttachment: (path: string) => void;
    clearAttachments: () => void;
    clearAttachedPaths: () => void;
    processRagDocuments: () => Promise<RagIndexResult | null>;
    searchRagContext: (query: string, limit?: number) => Promise<RagChunk[]>;
    clearRagContext: () => Promise<void>;
    fetchRagIndexedFiles: () => Promise<void>;
    removeRagFile: (sourceFile: string) => Promise<void>;
}

export const createRagSlice: StateCreator<
    RagSlice & RagSliceDeps,
    [],
    [],
    RagSlice
> = (set, get) => ({
    // RAG (Retrieval Augmented Generation) State
    attachedPaths: [],
    ragIndexedFiles: [],
    isIndexingRag: false,
    isSearchingRag: false,
    ragChunkCount: 0,
    
    addAttachment: async (path: string) => {
        const state = get();
        // Avoid duplicates
        if (state.attachedPaths.includes(path) || state.ragIndexedFiles.includes(path)) {
            return;
        }
        console.log(`[ChatStore] Adding attachment and indexing immediately: ${path}`);
        
        // Add path to attachedPaths
        set((s) => ({ attachedPaths: [...s.attachedPaths, path] }));
        
        // Immediately trigger indexing
        set({ 
            isIndexingRag: true,
            operationStatus: {
                type: 'indexing',
                message: 'Starting document processing...',
                startTime: Date.now(),
            }
        } as any);
        try {
            // Get the paths we're about to index
            const pathsToIndex = get().attachedPaths;
            const result = await invoke<RagIndexResult>('process_rag_documents', { paths: pathsToIndex });
            console.log(`[ChatStore] RAG indexing complete: ${result.total_chunks} chunks from ${result.files_processed} files`);

            // Check for errors
            if (result.file_errors && result.file_errors.length > 0) {
                const failedCount = result.file_errors.length;
                const successCount = pathsToIndex.length - failedCount;

                if (successCount === 0) {
                    // All files failed
                    set({
                        operationStatus: {
                            type: 'error',
                            message: `Failed to index: ${result.file_errors[0].error}`,
                            startTime: Date.now(),
                        },
                        isIndexingRag: false,
                        attachedPaths: [], // Clear pending paths since they failed
                        statusBarDismissed: false,
                    } as any);
                    return;
                } else {
                    // Partial success
                    set((s) => ({
                        ragChunkCount: s.ragChunkCount + result.total_chunks,
                        isIndexingRag: false,
                        attachedPaths: [],  // Clear pending paths
                        ragIndexedFiles: [...s.ragIndexedFiles, ...pathsToIndex.filter(p => !result.file_errors.some(fe => fe.file === p))],
                        operationStatus: {
                            type: 'indexing',
                            message: `Indexed ${successCount} file(s), ${failedCount} failed`,
                            startTime: Date.now(),
                            completed: true,
                        },
                        statusBarDismissed: false,
                    } as any));
                    return;
                }
            }

            // Full success - Update ragIndexedFiles with the paths we just indexed (append to existing)
            set((s) => ({
                ragChunkCount: s.ragChunkCount + result.total_chunks,
                isIndexingRag: false,
                attachedPaths: [],  // Clear pending paths
                ragIndexedFiles: [...s.ragIndexedFiles, ...pathsToIndex],  // Add newly indexed files
                operationStatus: null
            } as any));
        } catch (e: any) {
            console.error('[ChatStore] RAG processing failed:', e);
            set({ isIndexingRag: false, operationStatus: null } as any);
        }
    },
    
    removeAttachment: (path: string) => set((state) => {
        console.log(`[ChatStore] Removing attachment: ${path}`);
        return { attachedPaths: state.attachedPaths.filter(p => p !== path) };
    }),
    
    clearAttachments: () => {
        console.log('[ChatStore] Clearing all attachments');
        set({ attachedPaths: [], ragChunkCount: 0, ragIndexedFiles: [] });
        // Fire-and-forget clear of backend RAG context
        invoke<boolean>('clear_rag_context').catch(e => 
            console.error('[ChatStore] Failed to clear RAG context in clearAttachments:', e)
        );
    },
    
    clearAttachedPaths: () => {
        console.log('[ChatStore] Clearing attachment paths (preserving RAG context)');
        set({ attachedPaths: [] });
    },
    
    processRagDocuments: async () => {
        const paths = get().attachedPaths;
        if (paths.length === 0) {
            return null;
        }
        
        console.log(`[ChatStore] Processing ${paths.length} RAG documents...`);
        set({ 
            isIndexingRag: true,
            operationStatus: {
                type: 'indexing',
                message: 'Starting document processing...',
                startTime: Date.now(),
            }
        } as any);
        
        try {
            const result = await invoke<RagIndexResult>('process_rag_documents', { paths });
            console.log(`[ChatStore] RAG indexing complete: ${result.total_chunks} chunks from ${result.files_processed} files`);
            
            // Check for errors
            if (result.file_errors && result.file_errors.length > 0) {
                const failedCount = result.file_errors.length;
                const successCount = paths.length - failedCount;

                if (successCount === 0) {
                    // All files failed
                    set({
                        operationStatus: {
                            type: 'error',
                            message: `Failed to index: ${result.file_errors[0].error}`,
                            startTime: Date.now(),
                        },
                        isIndexingRag: false,
                        statusBarDismissed: false,
                    } as any);
                } else {
                    // Partial success
                    set((s) => ({
                        ragChunkCount: result.total_chunks,
                        isIndexingRag: false,
                        ragIndexedFiles: [...s.ragIndexedFiles, ...paths.filter(p => !result.file_errors.some(fe => fe.file === p))],
                        operationStatus: {
                            type: 'indexing',
                            message: `Indexed ${successCount} file(s), ${failedCount} failed`,
                            startTime: Date.now(),
                            completed: true,
                        },
                        statusBarDismissed: false,
                    } as any));
                }
            } else {
                // Full success
                set((s) => ({ 
                    ragChunkCount: result.total_chunks, 
                    isIndexingRag: false, 
                    ragIndexedFiles: [...s.ragIndexedFiles, ...paths],
                    operationStatus: null 
                } as any));
            }
            return result;
        } catch (e: any) {
            console.error('[ChatStore] RAG processing failed:', e);
            set({ isIndexingRag: false, operationStatus: null } as any);
            return null;
        }
    },
    
    searchRagContext: async (query: string, limit: number = 5) => {
        console.log(`[ChatStore] Searching RAG context for: "${query.slice(0, 50)}..."`);
        set({ isSearchingRag: true });
        
        try {
            const chunks = await invoke<RagChunk[]>('search_rag_context', { query, limit });
            console.log(`[ChatStore] Found ${chunks.length} relevant chunks`);
            set({ isSearchingRag: false });
            return chunks;
        } catch (e: any) {
            console.error('[ChatStore] RAG search failed:', e);
            set({ isSearchingRag: false });
            return [];
        }
    },
    
    clearRagContext: async () => {
        console.log('[ChatStore] Clearing RAG context');
        try {
            await invoke<boolean>('clear_rag_context');
            set({ attachedPaths: [], ragChunkCount: 0, ragIndexedFiles: [] });
        } catch (e: any) {
            console.error('[ChatStore] Failed to clear RAG context:', e);
        }
    },
    
    fetchRagIndexedFiles: async () => {
        try {
            const files = await invoke<string[]>('get_rag_indexed_files');
            console.log(`[ChatStore] Fetched ${files.length} indexed RAG files`);
            set({ ragIndexedFiles: files });
        } catch (e: any) {
            console.error('[ChatStore] Failed to fetch RAG indexed files:', e);
        }
    },
    
    removeRagFile: async (sourceFile: string) => {
        console.log(`[ChatStore] Removing RAG file: ${sourceFile}`);
        try {
            const result = await invoke<{ chunks_removed: number; remaining_chunks: number }>('remove_rag_file', { sourceFile });
            console.log(`[ChatStore] Removed ${result.chunks_removed} chunks, ${result.remaining_chunks} remaining`);
            // Update local state - remove the file from ragIndexedFiles and update chunk count
            set((s) => ({ 
                ragChunkCount: result.remaining_chunks,
                ragIndexedFiles: s.ragIndexedFiles.filter(f => f !== sourceFile)
            }));
        } catch (e: any) {
            console.error('[ChatStore] Failed to remove RAG file:', e);
        }
    },
});
